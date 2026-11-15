//! Staking emission: the genesis `validator_bootstrap` bucket (10% of the
//! supply by default, idle until now) is paid out over time to validators,
//! observers and delegators. Each epoch boundary moves
//! `remaining × emission.validator_bps_per_year × elapsed / year` from the
//! bucket into the system `validator_rewards` account, and the existing
//! epoch distribution (`staking::distribute_rewards`) pays it out pro rata
//! to power, delegators included. Nothing is minted: the supply cap holds,
//! the emission only decides who holds coins that already exist, and it
//! decays on its own because it is a share of what is left.
//!
//! Appended at the end of [`crate::State`] (schema 10, 2026-11-15). A
//! snapshot older than that migrates with the rate at 0, so replaying old
//! blocks is unchanged; governance turns it on with `SetParam`.

use crate::{
    context::BlockContext,
    modules::tokens,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_ledger::{Record, TxType};
use keel_types::Amount;
use serde::{Deserialize, Serialize};

/// 10% of the remaining bucket per year by default.
pub const DEFAULT_VALIDATOR_BPS_PER_YEAR: u32 = 1_000;
const YEAR_MS: u128 = 365 * 86_400_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct EmissionParams {
    /// `emission.validator_bps_per_year`: share of the bucket's remaining
    /// balance paid out per year, in basis points. 0 = off.
    pub validator_bps_per_year: u32,
}

impl Default for EmissionParams {
    fn default() -> Self {
        Self {
            validator_bps_per_year: DEFAULT_VALIDATOR_BPS_PER_YEAR,
        }
    }
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct EmissionState {
    pub params: EmissionParams,
    /// Block time (ms) the emission was last accrued to; 0 = never.
    pub last_paid_ms: u64,
    /// Total paid out of the bucket so far.
    pub paid_total: Amount,
}

impl EmissionState {
    /// What a chain that predates this module gets: nothing changes until
    /// governance sets the rate.
    pub fn migrated() -> Self {
        Self {
            params: EmissionParams {
                validator_bps_per_year: 0,
            },
            last_paid_ms: 0,
            paid_total: 0,
        }
    }
}

pub const PARAM_KEYS: &[&str] = &["emission.validator_bps_per_year"];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    match key {
        "emission.validator_bps_per_year" => {
            Some(state.emission.params.validator_bps_per_year as u128)
        }
        _ => None,
    }
}

pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    match key {
        "emission.validator_bps_per_year" => {
            state.emission.params.validator_bps_per_year = u32::try_from(value)
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

/// What is left in the bucket.
pub fn remaining(state: &State) -> Amount {
    let native = &state.tokens.native;
    state
        .ledger
        .balance(&tokens::system_key(native, "validator_bootstrap"))
        .max(0) as Amount
}

/// Accrue the emission for the time since the last accrual into
/// `validator_rewards`. Called at each epoch boundary before the rewards
/// are distributed, so the epoch pays what it earned.
pub fn accrue(state: &mut State, ctx: &BlockContext, epoch: u64) -> Vec<Event> {
    let bps = state.emission.params.validator_bps_per_year;
    let last = state.emission.last_paid_ms;
    if last == 0 || bps == 0 {
        // First boundary (or the rate is off): start the clock.
        state.emission.last_paid_ms = ctx.timestamp;
        return Vec::new();
    }
    let elapsed = ctx.timestamp.saturating_sub(last) as u128;
    state.emission.last_paid_ms = ctx.timestamp;
    let remaining = remaining(state);
    let amount = remaining
        .saturating_mul(bps as u128)
        .saturating_mul(elapsed)
        / (10_000 * YEAR_MS);
    if amount == 0 {
        return Vec::new();
    }
    let native = state.tokens.native.clone();
    let posted = state.ledger.post(
        &format!("emission:{epoch}"),
        TxType::Emission,
        None,
        None,
        vec![
            Record::debit(tokens::system_key(&native, "validator_bootstrap"), amount),
            Record::credit(tokens::system_key(&native, "validator_rewards"), amount),
        ],
    );
    if posted.is_err() {
        return Vec::new();
    }
    state.emission.paid_total = state.emission.paid_total.saturating_add(amount);
    vec![Event::EmissionPaid {
        epoch,
        amount,
        remaining: remaining.saturating_sub(amount),
    }]
}
