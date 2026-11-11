//! The pooled house-maker inventory (2026-10-02).
//!
//! The house maker quotes from one restricted system account per asset,
//! `SYSTEM/<asset>/swap_pool` (`markets::synthetic_levels` clamps every
//! quote to that balance). This module turns that account into a pool
//! anyone can fund:
//!
//! - `DepositLiquidity` moves an asset into `swap_pool` and mints shares on
//!   its USD value (KUSD at par, anything else at the last fill on its
//!   `<ASSET>-KUSD` book). The protocol itself holds shares for whatever the
//!   pool already held when the first depositor arrived (the genesis KEEL
//!   bucket).
//! - House fills grow the pool by the spread they capture. At every epoch
//!   the pool is marked at the same prices; `protocol_share_bps` of the
//!   gain above the high-water mark is taken in kind, pro rata across the
//!   pool's assets, through the ordinary fee split (treasury, validators,
//!   burn). The buyback that runs later in the same block turns those legs
//!   into KEEL. Depositors keep the rest as share value. Loss epochs take
//!   nothing and leave the mark.
//! - `WithdrawLiquidity` burns shares and moves the pro-rata slice of every
//!   pool asset to the owner's `liquidity_unlocking` account, released to
//!   the deposit after `withdraw_delay_blocks`. Pro rata in kind needs no
//!   price, so exits never depend on the mark. The coins leave `swap_pool`
//!   at once, so the next match sees the smaller clamp. The withdrawer's
//!   unrealised gain is crystallised on the way out when prices exist.
//!
//! Every behaviour here is gated on the `0.2.0` activation
//! (`gov::feature_active`), so blocks from before it replay unchanged.
//! Determinism: integer arithmetic through `mul_div_floor`, `BTreeMap`
//! iteration, no clock but the block.

use crate::{
    context::BlockContext,
    modules::{fees, gov, tokens},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::Action;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, pow10, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The node version whose activation switches the pool (and the client
/// pricing that shipped with it) on.
pub const ACTIVATION_VERSION: &str = "0.2.0";
/// The house party account type the pool lives in.
pub const SWAP_POOL: &str = "swap_pool";
/// Value per share is stored as `value_usd * VPS_SCALE / total_shares`.
pub const VPS_SCALE: u128 = 1_000_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct LiquidityParams {
    /// Share of the pool's gain above its high-water mark that goes to the
    /// protocol (fee split), in basis points.
    pub protocol_share_bps: u32,
    /// Smallest deposit, in USD micro-units.
    pub min_deposit_usd_micro: u64,
    /// Largest pool value after a deposit, in USD micro-units (0 = none).
    pub pool_cap_usd_micro: u64,
    /// Blocks between a withdrawal and its release to the deposit.
    pub withdraw_delay_blocks: u64,
    /// A house quote side may offer at most this share of the inventory.
    pub max_quote_inventory_bps: u32,
    /// Smallest `(ask - bid) / bid` a two-sided house quote may have (0 = none).
    pub min_spread_bps: u32,
    /// Most one mark may take, as a share of pool value: bounds what a
    /// manipulated last price can extract.
    pub max_epoch_fee_bps: u32,
}

impl Default for LiquidityParams {
    fn default() -> Self {
        Self {
            protocol_share_bps: 5_000,
            min_deposit_usd_micro: 10_000_000,
            pool_cap_usd_micro: 0,
            withdraw_delay_blocks: 10_000,
            max_quote_inventory_bps: 10_000,
            min_spread_bps: 0,
            max_epoch_fee_bps: 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Unlock {
    pub owner: Address,
    pub asset: Asset,
    pub amount: Amount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EpochRecord {
    pub height: u64,
    pub value_usd: Amount,
    pub shares: u128,
    pub value_per_share: u128,
    pub gain_usd: Amount,
    pub fee_usd: Amount,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Pool {
    pub total_shares: u128,
    /// Holder -> shares. `Address::SYSTEM` holds the protocol's own.
    pub shares: BTreeMap<Address, u128>,
    /// High-water mark of value per share (`VPS_SCALE` fixed point); 0 = unset.
    pub hwm: u128,
    /// Release height -> in-kind legs waiting there.
    pub unlocking: BTreeMap<u64, Vec<Unlock>>,
    pub last_epoch: Option<EpochRecord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct LiquidityState {
    pub params: LiquidityParams,
    /// Keyed by the house party account type; `swap_pool` today.
    pub pools: BTreeMap<String, Pool>,
}

pub const PARAM_KEYS: &[&str] = &[
    "liquidity.protocol_share_bps",
    "liquidity.min_deposit_usd_micro",
    "liquidity.pool_cap_usd_micro",
    "liquidity.withdraw_delay_blocks",
    "liquidity.max_quote_inventory_bps",
    "liquidity.min_spread_bps",
    "liquidity.max_epoch_fee_bps",
];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let p = &state.liquidity.params;
    Some(match key {
        "liquidity.protocol_share_bps" => p.protocol_share_bps as u128,
        "liquidity.min_deposit_usd_micro" => p.min_deposit_usd_micro as u128,
        "liquidity.pool_cap_usd_micro" => p.pool_cap_usd_micro as u128,
        "liquidity.withdraw_delay_blocks" => p.withdraw_delay_blocks as u128,
        "liquidity.max_quote_inventory_bps" => p.max_quote_inventory_bps as u128,
        "liquidity.min_spread_bps" => p.min_spread_bps as u128,
        "liquidity.max_epoch_fee_bps" => p.max_epoch_fee_bps as u128,
        _ => return None,
    })
}

/// `SetParam` / `ParamChange` for the `liquidity.*` keys.
pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    let bps = |v: u128| -> Result<u32, VmError> {
        u32::try_from(v)
            .ok()
            .filter(|b| *b <= 10_000)
            .ok_or_else(|| VmError::Invalid(format!("{key} must be within 0..=10000 bps")))
    };
    let u64v = |v: u128| -> Result<u64, VmError> {
        u64::try_from(v).map_err(|_| VmError::Invalid(format!("{key} too large")))
    };
    let p = &mut state.liquidity.params;
    match key {
        "liquidity.protocol_share_bps" => p.protocol_share_bps = bps(value)?,
        "liquidity.min_deposit_usd_micro" => p.min_deposit_usd_micro = u64v(value)?,
        "liquidity.pool_cap_usd_micro" => p.pool_cap_usd_micro = u64v(value)?,
        "liquidity.withdraw_delay_blocks" => {
            let v = u64v(value)?;
            if v > 1_000_000 {
                return Err(VmError::Invalid(format!("{key} must be <= 1000000")));
            }
            p.withdraw_delay_blocks = v;
        }
        "liquidity.max_quote_inventory_bps" => {
            let v = bps(value)?;
            if v == 0 {
                return Err(VmError::Invalid(format!("{key} must be > 0")));
            }
            p.max_quote_inventory_bps = v;
        }
        "liquidity.min_spread_bps" => p.min_spread_bps = bps(value)?,
        "liquidity.max_epoch_fee_bps" => p.max_epoch_fee_bps = bps(value)?,
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// Whether the pool (and everything gated with it) is live at `height`.
pub fn active(state: &State, height: u64) -> bool {
    gov::feature_active(state, height, ACTIVATION_VERSION)
}

fn pool_key(asset: &Asset) -> AccountKey {
    tokens::system_key(asset, SWAP_POOL)
}

fn unlocking_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "liquidity_unlocking").expect("catalog type")
}

/// Every asset the pool holds, with its balance, in asset order.
pub fn holdings(state: &State) -> Vec<(Asset, Amount)> {
    state
        .tokens
        .assets
        .keys()
        .filter_map(|a| {
            let b = state.ledger.balance(&pool_key(a)).max(0) as Amount;
            (b > 0).then(|| (a.clone(), b))
        })
        .collect()
}

/// USD micro value of `amount` of `asset` for the pool: the stable at par,
/// anything else at the last fill on `<ASSET>-KUSD`. Never the house mid,
/// which the house operator sets. `None` without a price.
pub fn price_usd(state: &State, asset: &Asset, amount: Amount) -> Option<Amount> {
    if *asset == state.tokens.stable {
        return Some(amount);
    }
    let decimals = state.tokens.decimals(asset)?;
    let symbol = format!("{}-{}", asset.symbol(), state.tokens.stable);
    let price = state.markets.pairs.get(&symbol)?.last_price?;
    mul_div_floor(amount, price, pow10(decimals))
}

/// The pool's USD value; `None` if any holding has no price.
pub fn pool_value_usd(state: &State) -> Option<Amount> {
    let mut total: Amount = 0;
    for (asset, balance) in holdings(state) {
        total = total.saturating_add(price_usd(state, &asset, balance)?);
    }
    Some(total)
}

fn vps(value_usd: Amount, total_shares: u128) -> u128 {
    if total_shares == 0 {
        return 0;
    }
    mul_div_floor(value_usd, VPS_SCALE, total_shares).unwrap_or(u128::MAX)
}

fn pool(state: &State) -> Option<&Pool> {
    state.liquidity.pools.get(SWAP_POOL)
}

/// Depositors' share of the pool's balance in `asset`: what counts as a
/// user liability (the protocol's own shares are platform money). Zero
/// before activation or without a pool.
pub fn user_pool_liability(state: &State, height: u64, asset: &Asset) -> Amount {
    if !active(state, height) {
        return 0;
    }
    let Some(p) = pool(state) else {
        return 0;
    };
    if p.total_shares == 0 {
        return 0;
    }
    let system = p.shares.get(&Address::SYSTEM).copied().unwrap_or(0);
    let users = p.total_shares.saturating_sub(system);
    let balance = state.ledger.balance(&pool_key(asset)).max(0) as Amount;
    mul_div_floor(balance, users, p.total_shares).unwrap_or(0)
}

/// Pending in-kind releases of `owner`: (release height, asset, amount).
pub fn pending_unlocks(state: &State, owner: &Address) -> Vec<(u64, Asset, Amount)> {
    let Some(p) = pool(state) else {
        return Vec::new();
    };
    p.unlocking
        .iter()
        .flat_map(|(h, legs)| {
            legs.iter()
                .filter(|u| u.owner == *owner)
                .map(move |u| (*h, u.asset.clone(), u.amount))
        })
        .collect()
}

/// `owner`'s shares and the pro-rata slice of every holding they stand for.
pub fn position(state: &State, owner: &Address) -> (u128, Vec<(Asset, Amount)>) {
    let Some(p) = pool(state) else {
        return (0, Vec::new());
    };
    let shares = p.shares.get(owner).copied().unwrap_or(0);
    if shares == 0 || p.total_shares == 0 {
        return (shares, Vec::new());
    }
    let legs = holdings(state)
        .into_iter()
        .map(|(a, b)| {
            let amt = mul_div_floor(b, shares, p.total_shares).unwrap_or(0);
            (a, amt)
        })
        .collect();
    (shares, legs)
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    if !active(state, ctx.height) {
        return Err(VmError::Invalid("liquidity not active".into()));
    }
    match action {
        Action::DepositLiquidity { asset, amount } => deposit(state, ctx, signer, tx_id, asset, *amount),
        Action::WithdrawLiquidity { shares } => withdraw(state, ctx, signer, tx_id, *shares),
        _ => Err(VmError::Invalid("not a liquidity action".into())),
    }
}

/// Take the protocol's share of the pool's gain above its high-water mark,
/// in kind, and advance the mark. `pool_value` is the pool's current USD
/// value. Returns `(gain_usd, fee_taken_usd)`; `(0, 0)` when there is no
/// gain or no shares. Never fails after a posting: `fees::collect` from a
/// restricted account that holds at least the leg cannot be refused.
fn crystallise(
    state: &mut State,
    tag: &str,
    pool_value: Amount,
) -> Result<(Amount, Amount), VmError> {
    let (total_shares, hwm, share_bps, cap_bps) = {
        let p = state.liquidity.params.clone();
        let pool = state.liquidity.pools.entry(SWAP_POOL.into()).or_default();
        (
            pool.total_shares,
            pool.hwm,
            p.protocol_share_bps as Amount,
            p.max_epoch_fee_bps as Amount,
        )
    };
    if total_shares == 0 || hwm == 0 {
        return Ok((0, 0));
    }
    let hwm_value = mul_div_floor(hwm, total_shares, VPS_SCALE).unwrap_or(Amount::MAX);
    let gain = pool_value.saturating_sub(hwm_value);
    if gain == 0 {
        return Ok((0, 0));
    }
    let fee_usd = mul_div_floor(gain, share_bps, 10_000)
        .unwrap_or(0)
        .min(mul_div_floor(pool_value, cap_bps, 10_000).unwrap_or(0));
    let mut taken_usd: Amount = 0;
    if fee_usd > 0 {
        for (asset, balance) in holdings(state) {
            let take = mul_div_floor(balance, fee_usd, pool_value).unwrap_or(0);
            if take == 0 {
                continue;
            }
            fees::collect(
                state,
                &format!("{tag}:fee:{}", asset.as_str()),
                TxType::LiquidityFee,
                Some(tag),
                pool_key(&asset),
                &asset,
                take,
            )?;
            taken_usd = taken_usd.saturating_add(price_usd(state, &asset, take).unwrap_or(0));
        }
    }
    let after = pool_value.saturating_sub(taken_usd);
    let pool = state.liquidity.pools.get_mut(SWAP_POOL).expect("pool exists");
    pool.hwm = vps(after, total_shares);
    Ok((gain, taken_usd))
}

fn deposit(
    state: &mut State,
    _ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    asset: &Asset,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    if !state.tokens.is_registered(asset) {
        return Err(VmError::NotFound(format!("asset {}", asset.as_str())));
    }
    let usd_value = price_usd(state, asset, amount)
        .ok_or_else(|| VmError::Invalid(format!("{} has no price", asset.as_str())))?;
    let params = state.liquidity.params.clone();
    if usd_value < params.min_deposit_usd_micro as Amount {
        return Err(VmError::Invalid(format!(
            "deposit below the minimum of {} USD micro",
            params.min_deposit_usd_micro
        )));
    }
    let pool_value = pool_value_usd(state)
        .ok_or_else(|| VmError::Invalid("pool holds an unpriced asset".into()))?;
    if params.pool_cap_usd_micro > 0
        && pool_value.saturating_add(usd_value) > params.pool_cap_usd_micro as Amount
    {
        return Err(VmError::Invalid("pool cap reached".into()));
    }
    // The depositor must be able to pay before anything is crystallised.
    if (state.ledger.balance(&tokens::deposit_key(signer, asset)).max(0) as Amount) < amount {
        return Err(VmError::Invalid("insufficient balance".into()));
    }
    let tag = format!("liq:dep:{}", tokens::hex(tx_id));
    // Settle what the protocol is owed so far, so the newcomer neither pays
    // for gains made before it joined nor dilutes them.
    let (_, fee_taken) = crystallise(state, &tag, pool_value)?;
    let pool_value = pool_value.saturating_sub(fee_taken);

    let (total_shares, worthless) = {
        let pool = state.liquidity.pools.entry(SWAP_POOL.into()).or_default();
        (pool.total_shares, pool.total_shares > 0 && pool_value == 0)
    };
    // Shares with nothing behind them would skim the new deposit.
    if worthless {
        let pool = state.liquidity.pools.get_mut(SWAP_POOL).expect("pool exists");
        pool.shares.clear();
        pool.total_shares = 0;
        pool.hwm = 0;
    }
    let total_shares = if worthless { 0 } else { total_shares };
    let (system_minted, minted) = if total_shares == 0 {
        // First depositor: the protocol owns what the pool already held.
        (pool_value, usd_value)
    } else {
        (
            0,
            mul_div_floor(usd_value, total_shares, pool_value).unwrap_or(0),
        )
    };
    if minted == 0 {
        return Err(VmError::Invalid("deposit too small for a share".into()));
    }

    state.ledger.post(
        &tag,
        TxType::LiquidityDeposit,
        Some(&tag),
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, asset), amount),
            Record::credit(pool_key(asset), amount),
        ],
    )?;

    let pool = state.liquidity.pools.get_mut(SWAP_POOL).expect("pool exists");
    if system_minted > 0 {
        *pool.shares.entry(Address::SYSTEM).or_insert(0) += system_minted;
        pool.total_shares = pool.total_shares.saturating_add(system_minted);
    }
    *pool.shares.entry(signer).or_insert(0) += minted;
    pool.total_shares = pool.total_shares.saturating_add(minted);
    if total_shares == 0 {
        pool.hwm = VPS_SCALE;
    }
    Ok(vec![Event::LiquidityDeposited {
        owner: signer,
        asset: asset.clone(),
        amount,
        usd_value,
        shares: minted,
    }])
}

fn withdraw(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    shares: u128,
) -> Result<Vec<Event>, VmError> {
    if shares == 0 {
        return Err(VmError::Invalid("shares must be > 0".into()));
    }
    let (held, total_shares) = {
        let Some(p) = pool(state) else {
            return Err(VmError::Invalid("no pool".into()));
        };
        (p.shares.get(&signer).copied().unwrap_or(0), p.total_shares)
    };
    if shares > held {
        return Err(VmError::Invalid(format!("only {held} shares held")));
    }
    let tag = format!("liq:wd:{}", tokens::hex(tx_id));
    // The leaver settles the protocol's share of the gain so far, when the
    // pool can be marked; an unpriced holding must never block an exit.
    let fee_usd = match pool_value_usd(state) {
        Some(v) => crystallise(state, &tag, v)?.1,
        None => 0,
    };
    let delay = state.liquidity.params.withdraw_delay_blocks;
    let release = ctx.height.saturating_add(delay);
    let mut legs = Vec::new();
    for (asset, balance) in holdings(state) {
        let leg = mul_div_floor(balance, shares, total_shares).unwrap_or(0);
        if leg == 0 {
            continue;
        }
        state.ledger.post(
            &format!("{tag}:{}", asset.as_str()),
            TxType::LiquidityWithdraw,
            Some(&tag),
            None,
            vec![
                Record::debit(pool_key(&asset), leg),
                Record::credit(unlocking_key(signer, &asset), leg),
            ],
        )?;
        legs.push(Unlock {
            owner: signer,
            asset,
            amount: leg,
        });
    }
    let pool = state.liquidity.pools.get_mut(SWAP_POOL).expect("pool exists");
    let left = held - shares;
    if left == 0 {
        pool.shares.remove(&signer);
    } else {
        pool.shares.insert(signer, left);
    }
    pool.total_shares = pool.total_shares.saturating_sub(shares);
    if !legs.is_empty() {
        pool.unlocking.entry(release).or_default().extend(legs);
    }
    Ok(vec![Event::LiquidityWithdrawQueued {
        owner: signer,
        shares,
        release_height: release,
        fee_usd,
    }])
}

pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    if !active(state, ctx.height) {
        return Vec::new();
    }
    let mut events = release_due(state, ctx);
    let len = state.params.epoch_length_blocks.max(1);
    if ctx.height == 0 || !ctx.height.is_multiple_of(len) {
        return events;
    }
    let Some(p) = pool(state) else {
        return events;
    };
    if p.total_shares == 0 {
        return events;
    }
    let Some(value) = pool_value_usd(state) else {
        return events;
    };
    let tag = format!("liq:epoch:{}", ctx.height);
    let (gain, fee) = crystallise(state, &tag, value).unwrap_or_default();
    let after = value.saturating_sub(fee);
    let pool = state.liquidity.pools.get_mut(SWAP_POOL).expect("pool exists");
    let shares = pool.total_shares;
    let value_per_share = vps(after, shares);
    pool.last_epoch = Some(EpochRecord {
        height: ctx.height,
        value_usd: after,
        shares,
        value_per_share,
        gain_usd: gain,
        fee_usd: fee,
    });
    if fee > 0 {
        events.push(Event::LiquidityFeeTaken {
            height: ctx.height,
            gain_usd: gain,
            fee_usd: fee,
        });
    }
    events.push(Event::LiquidityValued {
        height: ctx.height,
        value_usd: after,
        shares,
        value_per_share,
    });
    events
}

fn release_due(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let due: Vec<u64> = match pool(state) {
        Some(p) => p.unlocking.range(..=ctx.height).map(|(h, _)| *h).collect(),
        None => return events,
    };
    for h in due {
        let Some(legs) = state
            .liquidity
            .pools
            .get_mut(SWAP_POOL)
            .and_then(|p| p.unlocking.remove(&h))
        else {
            continue;
        };
        for (i, u) in legs.into_iter().enumerate() {
            let posted = state.ledger.post(
                &format!("liq:rel:{}:{}:{}", h, i, u.owner),
                TxType::LiquidityRelease,
                None,
                None,
                vec![
                    Record::debit(unlocking_key(u.owner, &u.asset), u.amount),
                    Record::credit(tokens::deposit_key(u.owner, &u.asset), u.amount),
                ],
            );
            if posted.is_ok() {
                events.push(Event::LiquidityUnlocked {
                    owner: u.owner,
                    asset: u.asset,
                    amount: u.amount,
                });
            }
        }
    }
    events
}
