//! Snapshot schema versions and the migration path between them.
//!
//! A snapshot on disk is `b"KEEL" | u32 schema (little endian) | payload`.
//! Snapshots written before the header existed have no header and are
//! schema 0. `upgrade` turns the payload of any known schema into the
//! current `State`; a schema newer than this binary knows is refused, so an
//! old node never loads a snapshot it cannot interpret.
//!
//! Rules when the layout of `State` (or anything it contains) changes:
//! 1. bump `SCHEMA`;
//! 2. keep a frozen copy of the previous top-level layout here as
//!    `StateV<n>` (only the structs that changed need a frozen copy; the
//!    others are shared);
//! 3. add the `n -> n+1` step to `upgrade`;
//! 4. add a fixture snapshot of schema `n` under `tests/fixtures/` and a
//!    case in `tests/migrate.rs` that loads it.
//!
//! Consensus-affecting behaviour changes are a separate matter: they are
//! gated on an activation height from `state.gov.upgrades`, so that
//! replaying old blocks stays byte-identical (see `docs/dev-rules.md`).

use crate::state::State;
use borsh::BorshDeserialize;

/// Current snapshot schema.
pub const SCHEMA: u32 = 3;

/// Magic prefix of a versioned snapshot.
pub const MAGIC: &[u8; 4] = b"KEEL";

/// Splits a snapshot into `(schema, payload)`. Headerless bytes are schema 0.
pub fn split(bytes: &[u8]) -> (u32, &[u8]) {
    if bytes.len() >= 8 && &bytes[..4] == MAGIC {
        let schema = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        (schema, &bytes[8..])
    } else {
        (0, bytes)
    }
}

/// Prefixes a payload with the current header.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&SCHEMA.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decodes a payload of the given schema and upgrades it to the current
/// layout. `None` when the bytes do not decode or the schema is unknown.
pub fn upgrade(schema: u32, payload: &[u8]) -> Option<State> {
    match schema {
        // Schema 0 (headerless) and schema 1 share the same layout; 1 only
        // added the header. Schema 2 appended `State.clients`, schema 3
        // appended `State.custody`.
        0 | 1 => StateV1::try_from_slice(payload).ok().map(State::from),
        2 => StateV2::try_from_slice(payload).ok().map(State::from),
        3 => State::try_from_slice(payload).ok(),
        _ => None,
    }
}

/// The top-level layout of schemas 0 and 1: everything up to `last_hash`.
/// Nested module states are shared with the current layout because none
/// of them changed.
#[derive(BorshDeserialize)]
pub struct StateV1 {
    pub chain_id: u32,
    pub height: u64,
    pub timestamp: u64,
    pub params: crate::params::Params,
    pub accounts: std::collections::BTreeMap<keel_types::Address, crate::state::AccountMeta>,
    pub ledger: keel_ledger::Ledger,
    pub tokens: crate::modules::tokens::TokensState,
    pub markets: crate::modules::markets::MarketsState,
    pub p2p: crate::modules::p2p::P2pState,
    pub disputes: crate::modules::disputes::DisputesState,
    pub vaults: crate::modules::vaults::VaultsState,
    pub stable: crate::modules::stable::StableState,
    pub staking: crate::modules::staking::StakingState,
    pub gov: crate::modules::gov::GovState,
    pub attest: crate::modules::attest::AttestState,
    pub budgets: crate::modules::budgets::BudgetsState,
    pub sessions: crate::modules::sessions::SessionsState,
    pub lightning: crate::modules::lightning::LightningState,
    pub paused: std::collections::BTreeMap<String, u64>,
    pub gov_house_operator: Option<keel_types::Address>,
    pub last_hash: keel_crypto::Hash32,
}

/// Schema 2: schema 1 plus `clients`.
#[derive(BorshDeserialize)]
pub struct StateV2 {
    pub chain_id: u32,
    pub height: u64,
    pub timestamp: u64,
    pub params: crate::params::Params,
    pub accounts: std::collections::BTreeMap<keel_types::Address, crate::state::AccountMeta>,
    pub ledger: keel_ledger::Ledger,
    pub tokens: crate::modules::tokens::TokensState,
    pub markets: crate::modules::markets::MarketsState,
    pub p2p: crate::modules::p2p::P2pState,
    pub disputes: crate::modules::disputes::DisputesState,
    pub vaults: crate::modules::vaults::VaultsState,
    pub stable: crate::modules::stable::StableState,
    pub staking: crate::modules::staking::StakingState,
    pub gov: crate::modules::gov::GovState,
    pub attest: crate::modules::attest::AttestState,
    pub budgets: crate::modules::budgets::BudgetsState,
    pub sessions: crate::modules::sessions::SessionsState,
    pub lightning: crate::modules::lightning::LightningState,
    pub paused: std::collections::BTreeMap<String, u64>,
    pub gov_house_operator: Option<keel_types::Address>,
    pub last_hash: keel_crypto::Hash32,
    pub clients: crate::modules::clients::ClientsState,
}

impl From<StateV2> for State {
    fn from(v: StateV2) -> Self {
        State {
            chain_id: v.chain_id,
            height: v.height,
            timestamp: v.timestamp,
            params: v.params,
            accounts: v.accounts,
            ledger: v.ledger,
            tokens: v.tokens,
            markets: v.markets,
            p2p: v.p2p,
            disputes: v.disputes,
            vaults: v.vaults,
            stable: v.stable,
            staking: v.staking,
            gov: v.gov,
            attest: v.attest,
            budgets: v.budgets,
            sessions: v.sessions,
            lightning: v.lightning,
            paused: v.paused,
            gov_house_operator: v.gov_house_operator,
            last_hash: v.last_hash,
            clients: v.clients,
            custody: Default::default(),
        }
    }
}

impl From<StateV1> for State {
    fn from(v: StateV1) -> Self {
        State {
            chain_id: v.chain_id,
            height: v.height,
            timestamp: v.timestamp,
            params: v.params,
            accounts: v.accounts,
            ledger: v.ledger,
            tokens: v.tokens,
            markets: v.markets,
            p2p: v.p2p,
            disputes: v.disputes,
            vaults: v.vaults,
            stable: v.stable,
            staking: v.staking,
            gov: v.gov,
            attest: v.attest,
            budgets: v.budgets,
            sessions: v.sessions,
            lightning: v.lightning,
            paused: v.paused,
            gov_house_operator: v.gov_house_operator,
            last_hash: v.last_hash,
            clients: Default::default(),
            custody: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_recognises_header_and_headerless() {
        let (s, p) = split(&[1, 2, 3]);
        assert_eq!((s, p), (0, &[1u8, 2, 3][..]));
        let framed = frame(&[9, 9]);
        let (s, p) = split(&framed);
        assert_eq!((s, p), (SCHEMA, &[9u8, 9][..]));
        // Too short to be a header, even with the magic.
        assert_eq!(split(b"KEEL").0, 0);
    }

    #[test]
    fn unknown_schema_is_refused() {
        let state = State::default();
        let payload = borsh::to_vec(&state).unwrap();
        assert!(upgrade(SCHEMA, &payload).is_some());
        assert!(upgrade(SCHEMA + 1, &payload).is_none());
    }
}
