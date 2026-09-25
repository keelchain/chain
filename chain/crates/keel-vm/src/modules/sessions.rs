//! Session keys (2026-09-09, non-custodial wallets): a principal
//! authorizes a scope-limited key that a site may hold, so order-book
//! trading and offer management need no wallet popup per click. A session
//! key can never move funds (`Action::session_scope`).

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::session_scope;
use keel_types::Address;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MAX_SESSIONS_PER_PRINCIPAL: usize = 16;
pub const MAX_SESSION_SECS: u64 = 30 * 86_400;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Session {
    pub principal: Address,
    pub scope: u32,
    /// Block time in seconds after which the key is dead.
    pub expires_at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct SessionsState {
    pub by_key: BTreeMap<Address, Session>,
}

/// Which principal a signer acts for, if the signer is a live session key
/// allowed to sign `scope_needed`. `Ok(None)` = not a session key.
pub fn principal_of(
    state: &State,
    signer: Address,
    action: &keel_actions::Action,
    now_secs: u64,
) -> Result<Option<Address>, VmError> {
    let Some(s) = state.sessions.by_key.get(&signer) else {
        return Ok(None);
    };
    // A session key may always revoke itself (also once expired, to clean up).
    if matches!(action, keel_actions::Action::RevokeSessionKey { key } if *key == signer) {
        return Ok(None);
    }
    if now_secs > s.expires_at {
        return Err(VmError::Invalid("session key expired".into()));
    }
    match action.session_scope() {
        Some(bit) if s.scope & bit == bit => Ok(Some(s.principal)),
        _ => Err(VmError::Unauthorized),
    }
}

pub fn authorize(
    state: &mut State,
    ctx: &BlockContext,
    principal: Address,
    key: Address,
    scope: u32,
    expires_at: u64,
) -> Result<Vec<Event>, VmError> {
    if key == principal || key.is_system() {
        return Err(VmError::Invalid(
            "session key must be a distinct, non-system address".into(),
        ));
    }
    if scope == 0 || scope & !session_scope::ALL != 0 {
        return Err(VmError::Invalid("unknown session scope bits".into()));
    }
    let now = ctx.seconds();
    if expires_at <= now || expires_at > now.saturating_add(MAX_SESSION_SECS) {
        return Err(VmError::Invalid(
            "expires_at must be in the future, at most 30 days ahead".into(),
        ));
    }
    let is_session = state.sessions.by_key.contains_key(&key);
    if !is_session
        && (state.accounts.contains_key(&key) || state.ledger.accounts_of(key).next().is_some())
    {
        // A key that already acts on its own (has a nonce or balances) is
        // someone's account, not a fresh session key.
        return Err(VmError::Invalid(
            "address already in use as an account".into(),
        ));
    }
    if let Some(existing) = state.sessions.by_key.get(&key) {
        if existing.principal != principal {
            return Err(VmError::Unauthorized);
        }
    }
    let live = state
        .sessions
        .by_key
        .values()
        .filter(|s| s.principal == principal && s.expires_at > now)
        .count();
    if live >= MAX_SESSIONS_PER_PRINCIPAL && !state.sessions.by_key.contains_key(&key) {
        return Err(VmError::Invalid("too many live session keys".into()));
    }
    state.sessions.by_key.insert(
        key,
        Session {
            principal,
            scope,
            expires_at,
        },
    );
    Ok(vec![Event::SessionAuthorized {
        principal,
        key,
        scope,
        expires_at,
    }])
}

pub fn revoke(state: &mut State, signer: Address, key: Address) -> Result<Vec<Event>, VmError> {
    let Some(s) = state.sessions.by_key.get(&key) else {
        return Err(VmError::Invalid("no such session key".into()));
    };
    if signer != s.principal && signer != key {
        return Err(VmError::Unauthorized);
    }
    let principal = s.principal;
    state.sessions.by_key.remove(&key);
    Ok(vec![Event::SessionRevoked { principal, key }])
}

/// Live sessions of a principal, for the RPC account view.
pub fn sessions_of(state: &State, principal: Address, now_secs: u64) -> Vec<(Address, Session)> {
    state
        .sessions
        .by_key
        .iter()
        .filter(|(_, s)| s.principal == principal && s.expires_at > now_secs)
        .map(|(k, s)| (*k, s.clone()))
        .collect()
}
