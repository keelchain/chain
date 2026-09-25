//! Fee routing. Every fee the chain earns is split at collection time into
//! treasury (the DAO's platform fee), validator rewards and burn, per the
//! governance split. One balanced posting, so the split is auditable per
//! fee.

use crate::{receipt::VmError, state::State};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, Amount, Asset};

use super::tokens::system_key;

pub struct Split {
    pub treasury: Amount,
    pub validators: Amount,
    pub burn: Amount,
}

pub fn split(state: &State, amount: Amount) -> Split {
    let p = &state.params;
    let validators =
        mul_div_floor(amount, p.fee_split_validators_bps as Amount, 10_000).unwrap_or(0);
    let burn = mul_div_floor(amount, p.fee_split_burn_bps as Amount, 10_000).unwrap_or(0);
    // Rounding dust goes to the treasury so nothing is lost.
    let treasury = amount.saturating_sub(validators).saturating_sub(burn);
    Split {
        treasury,
        validators,
        burn,
    }
}

/// Move `amount` of `asset` from `from` into the split destinations.
pub fn collect(
    state: &mut State,
    external_id: &str,
    tx_type: TxType,
    group_id: Option<&str>,
    from: AccountKey,
    asset: &Asset,
    amount: Amount,
) -> Result<(), VmError> {
    if amount == 0 {
        return Ok(());
    }
    let s = split(state, amount);
    let mut records = vec![Record::debit(from, amount)];
    if s.treasury > 0 {
        records.push(Record::credit(system_key(asset, "treasury"), s.treasury));
    }
    if s.validators > 0 {
        records.push(Record::credit(
            system_key(asset, "validator_rewards"),
            s.validators,
        ));
    }
    if s.burn > 0 {
        records.push(Record::credit(system_key(asset, "burn"), s.burn));
    }
    state
        .ledger
        .post(external_id, tx_type, group_id, None, records)?;
    Ok(())
}
