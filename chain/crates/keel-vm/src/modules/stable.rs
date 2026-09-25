//! The USD stablecoin (`state.tokens.stable`, ticker placeholder KUSD),
//! backed 1:1 by a governance-set basket of vaulted reserve assets.
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

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::Action;
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
    _ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::MintStable { asset, amount } => mint(state, signer, tx_id, asset, *amount),
        Action::BurnStable { asset, amount } => burn(state, signer, tx_id, asset, *amount),
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
    let group = format!("stable:{}", tokens::hex(tx_id));
    state.ledger.post(
        &format!("{group}:reserve"),
        TxType::StableMint,
        Some(&group),
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, asset), amount),
            Record::credit(tokens::system_key(asset, "stable_reserve"), amount),
        ],
    )?;
    state.ledger.post(
        &format!("{group}:mint"),
        TxType::StableMint,
        Some(&group),
        None,
        vec![
            Record::debit(tokens::system_key(&stable, "issuance"), minted),
            Record::credit(tokens::deposit_key(signer, &stable), minted),
        ],
    )?;
    let e = state.stable.basket.get_mut(asset).expect("checked");
    e.reserve = new_reserve;
    state.stable.supply = state.stable.supply.saturating_add(minted);
    Ok(vec![Event::StableMinted {
        owner: signer,
        from: asset.clone(),
        amount: minted,
    }])
}

fn burn(
    state: &mut State,
    signer: Address,
    tx_id: &[u8; 32],
    asset: &Asset,
    amount: Amount,
) -> Result<Vec<Event>, VmError> {
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
    Ok(vec![Event::StableBurned {
        owner: signer,
        into: asset.clone(),
        amount,
    }])
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
