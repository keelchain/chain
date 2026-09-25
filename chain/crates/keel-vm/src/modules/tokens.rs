//! Asset registry, transfers, USD valuation.

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Chain, Transfer};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, pow10, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub enum AssetKind {
    Native,
    Stable,
    Vault { chain: Chain },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct AssetInfo {
    pub decimals: u32,
    pub kind: AssetKind,
}

#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct TokensState {
    pub assets: BTreeMap<Asset, AssetInfo>,
    pub native: Asset,
    pub stable: Asset,
    /// KEEL vesting schedules by beneficiary (team and early contributors).
    pub vesting: BTreeMap<Address, Vesting>,
}

/// Cliff then linear release, measured in block time (seconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Vesting {
    pub total: Amount,
    pub released: Amount,
    pub start_secs: u64,
    pub cliff_secs: u64,
    pub duration_secs: u64,
}

impl Vesting {
    /// Amount vested by `now` (whole schedule once `duration` has passed).
    pub fn vested_at(&self, now: u64) -> Amount {
        if now < self.start_secs.saturating_add(self.cliff_secs) {
            return 0;
        }
        let elapsed = now.saturating_sub(self.start_secs);
        if self.duration_secs == 0 || elapsed >= self.duration_secs {
            return self.total;
        }
        mul_div_floor(self.total, elapsed as Amount, self.duration_secs as Amount)
            .unwrap_or(self.total)
    }
}

impl Default for TokensState {
    fn default() -> Self {
        Self {
            assets: BTreeMap::new(),
            native: Asset::new("KEEL"),
            stable: Asset::new("KUSD"),
            vesting: BTreeMap::new(),
        }
    }
}

impl TokensState {
    pub fn decimals(&self, asset: &Asset) -> Option<u32> {
        self.assets.get(asset).map(|a| a.decimals)
    }
    pub fn kind(&self, asset: &Asset) -> Option<&AssetKind> {
        self.assets.get(asset).map(|a| &a.kind)
    }
    pub fn is_registered(&self, asset: &Asset) -> bool {
        self.assets.contains_key(asset)
    }
}

pub fn register(state: &mut State, asset: Asset, decimals: u32, kind: AssetKind) {
    match &kind {
        AssetKind::Native => state.tokens.native = asset.clone(),
        AssetKind::Stable => state.tokens.stable = asset.clone(),
        AssetKind::Vault { .. } => {}
    }
    state
        .tokens
        .assets
        .insert(asset, AssetInfo { decimals, kind });
}

pub fn deposit_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "deposit").expect("deposit is a catalog type")
}

pub fn system_key(asset: &Asset, account_type: &str) -> AccountKey {
    AccountKey::new(Address::SYSTEM, asset.clone(), account_type).expect("catalog type")
}

/// Spendable balance of `owner` in `asset`.
pub fn balance(state: &State, owner: Address, asset: &Asset) -> Amount {
    state.ledger.balance(&deposit_key(owner, asset)).max(0) as Amount
}

/// Value in the stable asset's smallest units (USD micro), from the
/// market's last price for `{asset}-{stable}`. `None` when no price exists.
pub fn usd_value(state: &State, asset: &Asset, amount: Amount) -> Option<Amount> {
    if *asset == state.tokens.stable {
        return Some(amount);
    }
    let decimals = state.tokens.decimals(asset)?;
    let symbol = format!("{}-{}", asset.symbol(), state.tokens.stable);
    let price = state.markets.price_of(&symbol)?;
    mul_div_floor(amount, price, pow10(decimals))
}

pub fn apply_transfer(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    t: &Transfer,
) -> Result<Vec<Event>, VmError> {
    if !state.tokens.is_registered(&t.asset) {
        return Err(VmError::NotFound(format!("asset {}", t.asset)));
    }
    if t.amount == 0 {
        return Err(VmError::Invalid("amount must be > 0".into()));
    }
    if t.to == signer {
        return Err(VmError::Invalid("cannot transfer to self".into()));
    }
    if t.to.is_system() {
        return Err(VmError::Invalid(
            "cannot transfer to the system address".into(),
        ));
    }
    if t.memo.as_ref().is_some_and(|m| m.len() > 256) {
        return Err(VmError::Invalid("memo too long".into()));
    }
    let _ = ctx;
    state.ledger.post(
        &format!("transfer:{}", hex(tx_id)),
        TxType::InternalTransferComplete,
        None,
        None,
        vec![
            Record::debit(deposit_key(signer, &t.asset), t.amount),
            Record::credit(deposit_key(t.to, &t.asset), t.amount),
        ],
    )?;
    Ok(vec![Event::Transferred {
        from: signer,
        to: t.to,
        asset: t.asset.clone(),
        amount: t.amount,
    }])
}

/// Lock `total` KEEL from the system `vesting` account for `owner`. Genesis
/// funds the account first; the schedule releases from it.
pub fn add_vesting(state: &mut State, owner: Address, v: Vesting) {
    state.tokens.vesting.insert(owner, v);
}

/// Release whatever has vested since the last block. Cheap: one map walk
/// per block, and a posting only when something is due.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let now = ctx.seconds();
    let native = state.tokens.native.clone();
    let due: Vec<(Address, Amount)> = state
        .tokens
        .vesting
        .iter()
        .filter_map(|(owner, v)| {
            let releasable = v.vested_at(now).saturating_sub(v.released);
            (releasable > 0).then_some((*owner, releasable))
        })
        .collect();
    let mut events = Vec::new();
    for (owner, amount) in due {
        let posted = state.ledger.post(
            &format!("vesting:{owner}:{}", ctx.height),
            TxType::VestingRelease,
            None,
            None,
            vec![
                Record::debit(system_key(&native, "vesting"), amount),
                Record::credit(deposit_key(owner, &native), amount),
            ],
        );
        if posted.is_ok() {
            if let Some(v) = state.tokens.vesting.get_mut(&owner) {
                v.released = v.released.saturating_add(amount);
            }
            events.push(Event::VestingReleased { owner, amount });
        }
    }
    events
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
