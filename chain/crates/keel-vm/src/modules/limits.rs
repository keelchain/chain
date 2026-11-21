//! Governance-owned limits that used to be constants in the code: the
//! activation floor and the per-action fee of the budget system, and the
//! size and time caps of sessions, offers, disputes, proposals and
//! transfers. Every number here is a `ParamChange` key
//! (`docs/governance.md`); nothing in a module compares against a literal.
//!
//! The struct is module state appended at the end of [`crate::State`]
//! (schema 9, 2026-11-15), so `Params` keeps its layout. A snapshot older
//! than schema 9 migrates with [`LimitsState::migrated`]: every cap at the
//! value the code used to hard-wire and the two new behaviours
//! (activation floor, fee mode) off, so replaying old blocks is unchanged.

use crate::{
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// 0.1 KEEL: dust for a user (the faucet hands out whole coins, a client
/// sends it with the attestation), a real cost per key for a flood.
pub const DEFAULT_ACTIVATION_KEEL: u128 = 100_000;
/// No per-action fee: an address without earned or locked capacity cannot
/// act, it does not pay. Governance may set a price instead.
pub const DEFAULT_ACTION_FEE_KEEL: u128 = 0;
/// 1 KEEL: the least a `LockBudget` must lock to be admitted without
/// spending budget (how a fresh address gets its first capacity).
pub const DEFAULT_FREE_LOCK_MIN_KEEL: u128 = keel_types::ORCAS_PER_KEEL;
pub const DEFAULT_MAX_SESSIONS_PER_PRINCIPAL: u32 = 16;
pub const DEFAULT_MAX_SESSION_SECS: u64 = 30 * 86_400;
pub const DEFAULT_MIN_PAYMENT_WINDOW_SECS: u32 = 300;
pub const DEFAULT_MAX_TERMS_CHARS: u32 = 2_000;
pub const DEFAULT_MAX_MARGIN_BPS: u32 = 5_000;
pub const DEFAULT_MAX_EVIDENCE: u32 = 32;
pub const DEFAULT_MAX_TITLE_CHARS: u32 = 200;
pub const DEFAULT_MAX_DESCRIPTION_CHARS: u32 = 10_000;
pub const DEFAULT_MAX_MEMO_BYTES: u32 = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct LimitsState {
    /// `budget.activation_keel`: the least KEEL (in orcas; spendable,
    /// locked, unlocking, bonded, and offer or proposal deposits all count)
    /// an address must hold before the chain accepts a budget-spending
    /// action from it. The free budget is worth
    /// something, so a key must cost something: without this, a budget per
    /// address is a budget per keypair and the chain is open to a flood of
    /// fresh keys. Observer actions and cancels are exempt. 0 = off.
    pub activation_keel: u128,
    /// `budget.action_fee_keel`: what an action costs, in KEEL smallest
    /// units, once governance sets `budget.base` to 0 (fee mode). Charged
    /// through the fee split like every other fee, only when the address
    /// has no earned or locked capacity left. 0 in fee mode = an address
    /// may act only on earned or locked capacity.
    pub action_fee_keel: u128,
    /// `budget.free_lock_min_keel`: a `LockBudget` of at least this many
    /// orcas, which the signer can pay in full, is admitted without
    /// spending budget. That is the way in for an address with no
    /// capacity; each free lock ties its KEEL up for the unlock delay, so
    /// it cannot be repeated for nothing. 0 = no lock is free.
    pub free_lock_min_keel: u128,
    /// `sessions.max_per_principal`: live session keys one account may hold.
    pub max_sessions_per_principal: u32,
    /// `sessions.max_secs`: longest life of a session key.
    pub max_session_secs: u64,
    /// `p2p.min_payment_window_secs`: shortest payment window an offer may set.
    pub min_payment_window_secs: u32,
    /// `p2p.max_terms_chars`: longest offer terms text.
    pub max_terms_chars: u32,
    /// `p2p.max_margin_bps`: largest absolute margin an offer may quote.
    pub max_margin_bps: u32,
    /// `disputes.max_evidence`: evidence items per dispute.
    pub max_evidence: u32,
    /// `gov.max_title_chars`, `gov.max_description_chars`: proposal text.
    pub max_title_chars: u32,
    pub max_description_chars: u32,
    /// `transfer.max_memo_bytes`: longest transfer memo.
    pub max_memo_bytes: u32,
}

impl Default for LimitsState {
    /// Genesis values: every cap at its historical figure, the activation
    /// floor on, the fee at its default (dormant while `budget.base` > 0).
    fn default() -> Self {
        Self {
            activation_keel: DEFAULT_ACTIVATION_KEEL,
            action_fee_keel: DEFAULT_ACTION_FEE_KEEL,
            free_lock_min_keel: DEFAULT_FREE_LOCK_MIN_KEEL,
            max_sessions_per_principal: DEFAULT_MAX_SESSIONS_PER_PRINCIPAL,
            max_session_secs: DEFAULT_MAX_SESSION_SECS,
            min_payment_window_secs: DEFAULT_MIN_PAYMENT_WINDOW_SECS,
            max_terms_chars: DEFAULT_MAX_TERMS_CHARS,
            max_margin_bps: DEFAULT_MAX_MARGIN_BPS,
            max_evidence: DEFAULT_MAX_EVIDENCE,
            max_title_chars: DEFAULT_MAX_TITLE_CHARS,
            max_description_chars: DEFAULT_MAX_DESCRIPTION_CHARS,
            max_memo_bytes: DEFAULT_MAX_MEMO_BYTES,
        }
    }
}

pub const PARAM_KEYS: &[&str] = &[
    "budget.activation_keel",
    "budget.action_fee_keel",
    "budget.free_lock_min_keel",
    "sessions.max_per_principal",
    "sessions.max_secs",
    "p2p.min_payment_window_secs",
    "p2p.max_terms_chars",
    "p2p.max_margin_bps",
    "disputes.max_evidence",
    "gov.max_title_chars",
    "gov.max_description_chars",
    "transfer.max_memo_bytes",
];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let l = &state.limits;
    Some(match key {
        "budget.activation_keel" => l.activation_keel,
        "budget.action_fee_keel" => l.action_fee_keel,
        "budget.free_lock_min_keel" => l.free_lock_min_keel,
        "sessions.max_per_principal" => l.max_sessions_per_principal as u128,
        "sessions.max_secs" => l.max_session_secs as u128,
        "p2p.min_payment_window_secs" => l.min_payment_window_secs as u128,
        "p2p.max_terms_chars" => l.max_terms_chars as u128,
        "p2p.max_margin_bps" => l.max_margin_bps as u128,
        "disputes.max_evidence" => l.max_evidence as u128,
        "gov.max_title_chars" => l.max_title_chars as u128,
        "gov.max_description_chars" => l.max_description_chars as u128,
        "transfer.max_memo_bytes" => l.max_memo_bytes as u128,
        _ => return None,
    })
}

/// `ParamChange` for the keys above, with the bounds that keep
/// the chain usable (a cap of zero would refuse every offer, proposal or
/// session).
pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    fn u32_in(key: &str, value: u128, lo: u32, hi: u32) -> Result<u32, VmError> {
        u32::try_from(value)
            .ok()
            .filter(|v| (lo..=hi).contains(v))
            .ok_or_else(|| VmError::Invalid(format!("{key} must be within {lo}..={hi}")))
    }
    let l = &mut state.limits;
    match key {
        "budget.activation_keel" => l.activation_keel = value,
        "budget.action_fee_keel" => l.action_fee_keel = value,
        "budget.free_lock_min_keel" => l.free_lock_min_keel = value,
        "sessions.max_per_principal" => {
            l.max_sessions_per_principal = u32_in(key, value, 1, 1_000)?
        }
        "sessions.max_secs" => {
            l.max_session_secs = u64::try_from(value)
                .ok()
                .filter(|v| (60..=365 * 86_400).contains(v))
                .ok_or_else(|| VmError::Invalid(format!("{key} must be within 60..=1 year")))?
        }
        "p2p.min_payment_window_secs" => {
            l.min_payment_window_secs = u32_in(key, value, 0, 7 * 86_400)?
        }
        "p2p.max_terms_chars" => l.max_terms_chars = u32_in(key, value, 1, 100_000)?,
        "p2p.max_margin_bps" => l.max_margin_bps = u32_in(key, value, 0, 10_000)?,
        "disputes.max_evidence" => l.max_evidence = u32_in(key, value, 1, 10_000)?,
        "gov.max_title_chars" => l.max_title_chars = u32_in(key, value, 1, 10_000)?,
        "gov.max_description_chars" => l.max_description_chars = u32_in(key, value, 1, 1_000_000)?,
        "transfer.max_memo_bytes" => l.max_memo_bytes = u32_in(key, value, 0, 65_535)?,
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// Whether the chain is in fee mode: no free budget, so an address without
/// earned or locked capacity pays `budget.action_fee_keel` per action.
pub fn fee_mode(state: &State) -> bool {
    state.params.budget.base == 0
}
