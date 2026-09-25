//! Attestations: governance-registered attesters (the KYC'd marketplace,
//! for one) assert a tier for a subject without putting identity on chain.

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_types::Address;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct AttestState {
    pub attesters: BTreeSet<Address>,
}

pub fn apply_attest(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    subject: Address,
    tier: u8,
    expires_at: u64,
) -> Result<Vec<Event>, VmError> {
    if !state.attest.attesters.contains(&signer) {
        return Err(VmError::Unauthorized);
    }
    if expires_at <= ctx.seconds() {
        return Err(VmError::Invalid("attestation already expired".into()));
    }
    let meta = state.account(subject);
    meta.tier = tier;
    meta.tier_expires_at = expires_at;
    Ok(vec![Event::Attested { subject, tier }])
}

/// Effective tier now (0 once expired).
pub fn tier_of(state: &State, subject: &Address, now_secs: u64) -> u8 {
    match state.account_ref(subject) {
        Some(m) if m.tier_expires_at > now_secs => m.tier,
        _ => 0,
    }
}
