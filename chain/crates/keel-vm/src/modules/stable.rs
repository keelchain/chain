//! The unified USDT (`state.tokens.stable`, ticker `USDT`): one balance
//! backed 1:1 by a governance-set basket of each chain's vaulted USDT.
//!
//! Mint: the signer's reserve asset moves from their `deposit` into the
//! system `stable_reserve` account for that asset (restricted, so the
//! reserve can never go negative), and the equivalent amount of the stable
//! is issued from the system `issuance` account into the signer's deposit.
//! Burn (`BurnStable { asset, amount }`): `amount` is in STABLE units; the
//! equivalent of the chosen reserve asset is paid back out of that asset's
//! reserve and the stable goes back to `issuance`.
//!
//! Invariant: stable supply <= sum over the basket of reserves converted to
//! stable units. Amounts convert by decimals, flooring, so rounding only
//! ever leaves reserve behind, never stable unbacked.
//!
//! One USDT (2026-11-15). The way an exchange shows one USDT balance
//! whatever network it arrived on: a deposit of a basket asset (USDT on
//! Ethereum, Tron or BSC) is converted into the unified USDT as it is
//! credited (`auto_convert`), and `WithdrawStable` redeems it into the
//! network's USDT the user picks and pays it out in one step. The user
//! never holds the per-chain asset. When a basket entry is disabled or at
//! its cap the deposit stays as credited. USDC is not USDT and is not in
//! the basket: it stays `SOL.USDC` with its own book.
//!
//! Rebalancing without a moderator. A chain's reserve can only pay out what
//! it holds, and nobody at Keel moves coins between chains. Each enabled
//! basket asset has a target share of the reserve (its cap's share of the
//! caps); a withdrawal through an asset below its target pays a premium of
//! up to `stable.rebalance_max_premium_bps`, scaled by how far below it is,
//! into the `stable_rebalance` pool, and a deposit of that asset earns the
//! same rate back out of the pool as a rebate. The gap is an arbitrage that
//! refills the short chain. Both knobs are governance parameters; a chain
//! that predates this migrates with conversion and premium off.

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, Withdraw, WithdrawStable};
use keel_ledger::{Record, TxType};
use keel_types::{pow10, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::tokens;

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct BasketEntry {
    pub cap: Amount,
    pub enabled: bool,
    /// Reserve held, in the reserve asset's smallest units.
    pub reserve: Amount,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct StableState {
    pub basket: BTreeMap<Asset, BasketEntry>,
    /// Stable units in circulation (minted minus burned).
    pub supply: Amount,
}

/// Governance-owned conversion and rebalancing knobs; module state appended
/// in schema 12 so `StableState` keeps its layout.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ConversionParams {
    /// `stable.auto_convert`: 1 = a credited deposit of a basket asset
    /// becomes USDT at once; 0 = deposits stay native (mint by hand).
    pub auto_convert: u32,
    /// `stable.rebalance_max_premium_bps`: premium on a `WithdrawStable`
    /// through an asset whose reserve share is zero, scaled linearly down
    /// to nothing at its target share; the same rate is the rebate a
    /// deposit of that asset earns from the pool. 0 = off.
    pub rebalance_max_premium_bps: u32,
}

impl Default for ConversionParams {
    fn default() -> Self {
        Self {
            auto_convert: 1,
            rebalance_max_premium_bps: 100,
        }
    }
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct ConversionState {
    pub params: ConversionParams,
}

impl ConversionState {
    /// What a chain that predates the module gets: nothing changes until
    /// governance turns conversion on.
    pub fn migrated() -> Self {
        Self {
            params: ConversionParams {
                auto_convert: 0,
                rebalance_max_premium_bps: 0,
            },
        }
    }
}

pub const PARAM_KEYS: &[&str] = &["stable.auto_convert", "stable.rebalance_max_premium_bps"];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let p = &state.stable_conversion.params;
    match key {
        "stable.auto_convert" => Some(p.auto_convert as u128),
        "stable.rebalance_max_premium_bps" => Some(p.rebalance_max_premium_bps as u128),
        _ => None,
    }
}

pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    let p = &mut state.stable_conversion.params;
    match key {
        "stable.auto_convert" => {
            p.auto_convert = u32::try_from(value)
                .ok()
                .filter(|v| *v <= 1)
                .ok_or_else(|| VmError::Invalid(format!("{key} must be 0 or 1")))?
        }
        "stable.rebalance_max_premium_bps" => {
            p.rebalance_max_premium_bps = u32::try_from(value)
                .ok()
                .filter(|v| *v <= 10_000)
                .ok_or_else(|| VmError::Invalid(format!("{key} must be within 0..=10000")))?
        }
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// The share of the caps `asset` is meant to hold and the share of the
/// reserve it holds, both in parts per million over the enabled basket.
pub fn shares_ppm(state: &State, asset: &Asset) -> Option<(u64, u64)> {
    let sd = state.tokens.decimals(&state.tokens.stable).unwrap_or(6);
    let in_stable = |a: &Asset, v: Amount| convert(v, state.tokens.decimals(a).unwrap_or(sd), sd);
    let entry = state.stable.basket.get(asset)?;
    if !entry.enabled {
        return None;
    }
    let (mut caps, mut reserves) = (0u128, 0u128);
    for (a, e) in state.stable.basket.iter().filter(|(_, e)| e.enabled) {
        caps = caps.saturating_add(in_stable(a, e.cap));
        reserves = reserves.saturating_add(in_stable(a, e.reserve));
    }
    if caps == 0 {
        return None;
    }
    let target = in_stable(asset, entry.cap).saturating_mul(1_000_000) / caps;
    let share = in_stable(asset, entry.reserve)
        .saturating_mul(1_000_000)
        .checked_div(reserves)
        .unwrap_or(0);
    Some((target as u64, share as u64))
}

/// Premium (bps of the amount) a withdrawal through `asset` pays now, and
/// the rebate rate a deposit of it earns: the maximum scaled by how far the
/// asset's reserve share is below its target share.
pub fn premium_bps(state: &State, asset: &Asset) -> u32 {
    let max = state.stable_conversion.params.rebalance_max_premium_bps as u128;
    if max == 0 {
        return 0;
    }
    match shares_ppm(state, asset) {
        Some((target, share)) if target > 0 && share < target => {
            (max * (target - share) as u128 / target as u128) as u32
        }
        _ => 0,
    }
}

/// The rebalancing pool: premiums collected, not yet paid out as rebates.
pub fn rebalance_pool(state: &State) -> Amount {
    state
        .ledger
        .balance(&tokens::system_key(
            &state.tokens.stable,
            "stable_rebalance",
        ))
        .max(0) as Amount
}

/// Governance hook: basket membership and cap for a reserve asset.
pub fn set_basket(state: &mut State, asset: Asset, cap: Amount, enabled: bool) {
    let e = state.stable.basket.entry(asset).or_default();
    e.cap = cap;
    e.enabled = enabled;
}

/// Convert between a reserve asset and the stable by decimals (floored).
fn convert(amount: Amount, from_decimals: u32, to_decimals: u32) -> Amount {
    if from_decimals == to_decimals {
        amount
    } else if from_decimals < to_decimals {
        amount.saturating_mul(pow10(to_decimals - from_decimals))
    } else {
        amount / pow10(from_decimals - to_decimals)
    }
}

/// Total reserves expressed in stable units.
pub fn reserves_in_stable(state: &State) -> Amount {
    let sd = state.tokens.decimals(&state.tokens.stable).unwrap_or(6);
    state
        .stable
        .basket
        .iter()
        .map(|(a, e)| convert(e.reserve, state.tokens.decimals(a).unwrap_or(sd), sd))
        .fold(0, Amount::saturating_add)
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::MintStable { asset, amount } => mint(state, signer, tx_id, asset, *amount),
        Action::BurnStable { asset, amount } => {
            burn(state, signer, tx_id, asset, *amount).map(|(_, e)| e)
        }
        Action::WithdrawStable(w) => withdraw_stable(state, ctx, signer, tx_id, w),
        _ => Err(VmError::Invalid("not a stable action".into())),
    }
}

fn decimals(state: &State, asset: &Asset) -> Result<(u32, u32), VmError> {
    let rd = state
        .tokens
        .decimals(asset)
        .ok_or_else(|| VmError::NotFound(format!("asset {asset}")))?;
    let sd = state
        .tokens
        .decimals(&state.tokens.stable)
        .ok_or_else(|| VmError::Invalid("stable not registered".into()))?;
    Ok((rd, sd))
}

fn mint(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    asset: &Asset,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
    let group = format!("stable:{}", tokens::hex(tx_id));
    mint_into(state, signer, &group, asset, amount).map(|(_, e)| e)
}

/// Move `amount` of `asset` from `owner`'s deposit into the reserve and
/// issue the stable to `owner`. Returns the stable minted.
fn mint_into(
    state: &mut State,
    signer: Address,
    group: &str,
    asset: &Asset,
    amount: Amount,
) -> Result<(Amount, Vec<Event>), VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let entry = state
        .stable
        .basket
        .get(asset)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("{asset} is not in the basket")))?;
    if !entry.enabled {
        return Err(VmError::Invalid("reserve asset disabled".into()));
    }
    let new_reserve = entry
        .reserve
        .checked_add(amount)
        .ok_or_else(|| VmError::Invalid("overflow".into()))?;
    if new_reserve > entry.cap {
        return Err(VmError::Invalid(
            "basket cap reached for this reserve asset".into(),
        ));
    }
    let (rd, sd) = decimals(state, asset)?;
    let minted = convert(amount, rd, sd);
    if minted == 0 {
        return Err(VmError::Invalid(
            "amount too small to mint one stable unit".into(),
        ));
    }
    let stable = state.tokens.stable.clone();
    state.ledger.post(
        &format!("{group}:reserve"),
        TxType::StableMint,
        Some(group),
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, asset), amount),
            Record::credit(tokens::system_key(asset, "stable_reserve"), amount),
        ],
    )?;
    state.ledger.post(
        &format!("{group}:mint"),
        TxType::StableMint,
        Some(group),
        None,
        vec![
            Record::debit(tokens::system_key(&stable, "issuance"), minted),
            Record::credit(tokens::deposit_key(signer, &stable), minted),
        ],
    )?;
    let e = state.stable.basket.get_mut(asset).expect("checked");
    e.reserve = new_reserve;
    state.stable.supply = state.stable.supply.saturating_add(minted);
    Ok((
        minted,
        vec![Event::StableMinted {
            owner: signer,
            from: asset.clone(),
            amount: minted,
        }],
    ))
}

/// A deposit of `asset` was just credited to `owner`: turn it into USDT
/// when conversion is on and the basket takes it, and pay the rebate the
/// asset's reserve shortfall earns. `None` leaves the deposit native (the
/// asset is not in the basket, is disabled, is at its cap, or conversion
/// is off); the caller changes nothing in that case.
pub fn auto_convert(
    state: &mut State,
    owner: Address,
    asset: &Asset,
    amount: Amount,
    external_id: &str,
) -> Option<Vec<Event>> {
    if state.stable_conversion.params.auto_convert == 0 {
        return None;
    }
    let entry = state.stable.basket.get(asset)?;
    if !entry.enabled || entry.reserve.saturating_add(amount) > entry.cap {
        return None;
    }
    // The rebate rate is the shortfall before this deposit refills it.
    let rate = premium_bps(state, asset);
    let group = format!("convert:{external_id}");
    let (minted, mut events) = mint_into(state, owner, &group, asset, amount).ok()?;
    let pool = rebalance_pool(state);
    let rebate = (minted.saturating_mul(rate as Amount) / 10_000).min(pool);
    if rebate > 0 {
        let stable = state.tokens.stable.clone();
        if state
            .ledger
            .post(
                &format!("{group}:rebate"),
                TxType::StableRebate,
                Some(&group),
                None,
                vec![
                    Record::debit(tokens::system_key(&stable, "stable_rebalance"), rebate),
                    Record::credit(tokens::deposit_key(owner, &stable), rebate),
                ],
            )
            .is_ok()
        {
            events.push(Event::StableRebatePaid {
                owner,
                from: asset.clone(),
                amount: rebate,
            });
        }
    }
    Some(events)
}

/// `WithdrawStable`: charge the premium of `via`, redeem the USDT into
/// `via`, and queue the withdrawal of the proceeds. One action, atomic.
fn withdraw_stable(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    w: &WithdrawStable,
) -> Result<Vec<Event>, VmError> {
    if w.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let mut events = Vec::new();
    let rate = premium_bps(state, &w.via);
    let premium = w.amount.saturating_mul(rate as Amount) / 10_000;
    if premium > 0 {
        let stable = state.tokens.stable.clone();
        state.ledger.post(
            &format!("stable:{}:premium", tokens::hex(tx_id)),
            TxType::StablePremium,
            None,
            None,
            vec![
                Record::debit(tokens::deposit_key(signer, &stable), premium),
                Record::credit(tokens::system_key(&stable, "stable_rebalance"), premium),
            ],
        )?;
        events.push(Event::StablePremiumPaid {
            owner: signer,
            via: w.via.clone(),
            amount: premium,
        });
    }
    let (reserve_out, burned) = burn(state, signer, tx_id, &w.via, w.amount)?;
    events.extend(burned);
    // The user spends exactly `amount` (plus the premium): the flat fee and,
    // when it is charged in the same asset, the network fee come out of the
    // redeemed coins, and the rest is what reaches the destination.
    let (flat, fee_asset, fee_estimate) =
        super::vaults::withdrawal_fees(state, ctx, signer, &w.via)?;
    let fees = flat.saturating_add(if fee_asset == w.via { fee_estimate } else { 0 });
    let payout = reserve_out
        .checked_sub(fees)
        .filter(|p| *p > 0)
        .ok_or_else(|| VmError::Invalid("amount does not cover the withdrawal fees".into()))?;
    events.extend(super::vaults::withdraw(
        state,
        ctx,
        signer,
        tx_id,
        &Withdraw {
            asset: w.via.clone(),
            to: w.to.clone(),
            amount: payout,
        },
    )?);
    Ok(events)
}

/// Redeem `amount` stable into `asset`, paid to `signer`'s deposit.
/// Returns the reserve units paid out.
fn burn(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    asset: &Asset,
    amount: Amount,
) -> Result<(Amount, Vec<Event>), VmError> {
    if amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    let entry = state
        .stable
        .basket
        .get(asset)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("{asset} is not in the basket")))?;
    let (rd, sd) = decimals(state, asset)?;
    let reserve_out = convert(amount, sd, rd);
    if reserve_out == 0 {
        return Err(VmError::Invalid(
            "amount too small to redeem one reserve unit".into(),
        ));
    }
    if reserve_out > entry.reserve {
        return Err(VmError::Invalid("not enough reserve in this asset".into()));
    }
    let stable = state.tokens.stable.clone();
    let group = format!("stable:{}", tokens::hex(tx_id));
    state.ledger.post(
        &format!("{group}:burn"),
        TxType::StableBurn,
        Some(&group),
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, &stable), amount),
            Record::credit(tokens::system_key(&stable, "issuance"), amount),
        ],
    )?;
    state.ledger.post(
        &format!("{group}:redeem"),
        TxType::StableBurn,
        Some(&group),
        None,
        vec![
            Record::debit(tokens::system_key(asset, "stable_reserve"), reserve_out),
            Record::credit(tokens::deposit_key(signer, asset), reserve_out),
        ],
    )?;
    let e = state.stable.basket.get_mut(asset).expect("checked");
    e.reserve = e.reserve.saturating_sub(reserve_out);
    state.stable.supply = state.stable.supply.saturating_sub(amount);
    Ok((
        reserve_out,
        vec![Event::StableBurned {
            owner: signer,
            into: asset.clone(),
            amount,
        }],
    ))
}

pub fn end_block(_state: &mut State, _ctx: &BlockContext) -> Vec<Event> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_floors_and_scales() {
        assert_eq!(convert(1_500_000, 6, 6), 1_500_000);
        assert_eq!(convert(1_500_000, 6, 18), 1_500_000 * pow10(12));
        assert_eq!(convert(1_999_999, 18, 6), 0);
        assert_eq!(convert(pow10(18) + 999_999_999_999, 18, 6), 1_000_000);
    }
}
