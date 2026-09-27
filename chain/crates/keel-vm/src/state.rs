use crate::{modules::*, params::Params};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::Budget;
use keel_crypto::{sha256, Hash32};
use keel_ledger::Ledger;
use keel_types::Address;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Per-address bookkeeping outside the ledger.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct AccountMeta {
    pub nonce: u64,
    pub budget: Budget,
    /// Highest attestation tier held (0 = none).
    pub tier: u8,
    pub tier_expires_at: u64,
}

/// The whole chain state. Serializable so a snapshot is one borsh blob
/// and the state hash is one sha256 over it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct State {
    pub chain_id: u32,
    pub height: u64,
    pub timestamp: u64,
    pub params: Params,
    pub accounts: BTreeMap<Address, AccountMeta>,
    pub ledger: Ledger,
    pub tokens: tokens::TokensState,
    pub markets: markets::MarketsState,
    pub p2p: p2p::P2pState,
    pub disputes: disputes::DisputesState,
    pub vaults: vaults::VaultsState,
    pub stable: stable::StableState,
    pub staking: staking::StakingState,
    pub gov: gov::GovState,
    pub attest: attest::AttestState,
    pub budgets: budgets::BudgetsState,
    pub sessions: sessions::SessionsState,
    pub lightning: lightning::LightningState,
    /// Module name -> height until which it is paused.
    pub paused: BTreeMap<String, u64>,
    /// Account allowed to post house quotes for system-owned pairs.
    pub gov_house_operator: Option<Address>,
    /// Hash after the last applied block.
    pub last_hash: Hash32,
    /// Client (attester) retail fees, who attested whom, usage prices and
    /// the treasury buyback knobs. Appended in schema 2.
    pub clients: clients::ClientsState,
    /// Client-owned custody vaults (schema 3).
    pub custody: custody::CustodyState,
}

impl State {
    pub fn account(&mut self, address: Address) -> &mut AccountMeta {
        self.accounts.entry(address).or_default()
    }

    pub fn account_ref(&self, address: &Address) -> Option<&AccountMeta> {
        self.accounts.get(address)
    }

    pub fn is_paused(&self, module: &str) -> bool {
        self.paused
            .get(module)
            .is_some_and(|until| *until > self.height)
    }

    /// Full-state hash, O(state). Used for genesis, snapshots and audits;
    /// per-block agreement uses the incremental commitment in `apply.rs`.
    pub fn compute_hash(&self) -> Hash32 {
        let mut copy = self.clone();
        copy.last_hash = [0u8; 32];
        let bytes = borsh::to_vec(&copy).unwrap_or_default();
        sha256(&[b"keel-state-v1", &bytes])
    }

    /// Versioned snapshot: `b"KEEL" | u32 schema | borsh(State)`; see
    /// `migrate.rs` for the schema rules.
    pub fn snapshot(&self) -> Vec<u8> {
        crate::migrate::frame(&borsh::to_vec(self).unwrap_or_default())
    }

    /// Loads a snapshot of any known schema (headerless bytes are schema 0)
    /// and upgrades it to the current layout.
    pub fn restore(bytes: &[u8]) -> Option<Self> {
        let (schema, payload) = crate::migrate::split(bytes);
        crate::migrate::upgrade(schema, payload)
    }
}
