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
pub const SCHEMA: u32 = 8;

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
        // appended `State.custody`, schema 4 appended `State.arbitration`,
        // schema 5 appended `State.pools`, schema 6 appended
        // `State.liquidity` and new fields inside `clients`, schema 7 added
        // `external_tx` inside `vaults`/`custody` (attestations, outbounds),
        // schema 8 appended `State.recovery`.
        0 | 1 => StateV1::try_from_slice(payload).ok().map(State::from),
        2 => StateV2::try_from_slice(payload).ok().map(State::from),
        3 => StateV3::try_from_slice(payload).ok().map(State::from),
        4 => StateV4::try_from_slice(payload).ok().map(State::from),
        5 => StateV5::try_from_slice(payload).ok().map(State::from),
        6 => StateV6::try_from_slice(payload).ok().map(State::from),
        7 => StateV7::try_from_slice(payload).ok().map(State::from),
        8 => State::try_from_slice(payload).ok(),
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
    pub vaults: VaultsStateV6,
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

/// Schema 2: schema 1 plus `clients` (in its schema-3 layout).
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
    pub vaults: VaultsStateV6,
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
    pub clients: crate::modules::clients::ClientsStateV3,
}

/// Schema 3: schema 2 plus `custody`.
#[derive(BorshDeserialize)]
pub struct StateV3 {
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
    pub vaults: VaultsStateV6,
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
    pub clients: crate::modules::clients::ClientsStateV3,
    pub custody: CustodyStateV6,
}

/// Schema 4: schema 3 plus `arbitration`.
#[derive(BorshDeserialize)]
pub struct StateV4 {
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
    pub vaults: VaultsStateV6,
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
    pub clients: crate::modules::clients::ClientsStateV3,
    pub custody: CustodyStateV6,
    pub arbitration: crate::modules::arbitration::ArbitrationState,
}

/// Schema 5: schema 4 plus `pools`; `clients` still in its schema-3
/// layout.
#[derive(BorshDeserialize)]
pub struct StateV5 {
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
    pub vaults: VaultsStateV6,
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
    pub clients: crate::modules::clients::ClientsStateV3,
    pub custody: CustodyStateV6,
    pub arbitration: crate::modules::arbitration::ArbitrationState,
    pub pools: crate::modules::pools::PoolsState,
}

impl From<StateV5> for State {
    fn from(v: StateV5) -> Self {
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
            vaults: v.vaults.into(),
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
            clients: v.clients.into(),
            custody: v.custody.into(),
            arbitration: v.arbitration,
            pools: v.pools,
            liquidity: Default::default(),
            recovery: Default::default(),
        }
    }
}

impl From<StateV4> for State {
    fn from(v: StateV4) -> Self {
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
            vaults: v.vaults.into(),
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
            clients: v.clients.into(),
            custody: v.custody.into(),
            arbitration: v.arbitration,
            pools: Default::default(),
            liquidity: Default::default(),
            recovery: Default::default(),
        }
    }
}

impl From<StateV3> for State {
    fn from(v: StateV3) -> Self {
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
            vaults: v.vaults.into(),
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
            clients: v.clients.into(),
            custody: v.custody.into(),
            arbitration: Default::default(),
            pools: Default::default(),
            liquidity: Default::default(),
            recovery: Default::default(),
        }
    }
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
            vaults: v.vaults.into(),
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
            clients: v.clients.into(),
            custody: Default::default(),
            arbitration: Default::default(),
            pools: Default::default(),
            liquidity: Default::default(),
            recovery: Default::default(),
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
            vaults: v.vaults.into(),
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
            arbitration: Default::default(),
            pools: Default::default(),
            liquidity: Default::default(),
            recovery: Default::default(),
        }
    }
}

/// Schema 6: schema 5 plus `liquidity`, with the current `clients`
/// layout but the schema-6 `vaults`/`custody` layouts (no `external_tx`).
#[derive(BorshDeserialize)]
pub struct StateV6 {
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
    pub vaults: VaultsStateV6,
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
    pub custody: CustodyStateV6,
    pub arbitration: crate::modules::arbitration::ArbitrationState,
    pub pools: crate::modules::pools::PoolsState,
    pub liquidity: crate::modules::liquidity::LiquidityState,
}

impl From<StateV6> for State {
    fn from(v: StateV6) -> Self {
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
            vaults: v.vaults.into(),
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
            custody: v.custody.into(),
            arbitration: v.arbitration,
            pools: v.pools,
            liquidity: v.liquidity,
            recovery: Default::default(),
        }
    }
}

// ---- the schema-6 (and older) vault layouts: attestations and outbounds
// without `external_tx` (schema 7) ----

/// `keel_attest::Attestation` before `external_tx`.
#[derive(BorshDeserialize)]
pub struct AttestationV6 {
    pub chain: keel_actions::Chain,
    pub tx_hash: keel_crypto::Hash32,
    pub index: u32,
    pub deposit_index: u64,
    pub amount: keel_types::Amount,
    pub asset: keel_types::Asset,
    pub external_height: u64,
}

impl From<AttestationV6> for keel_attest::Attestation {
    fn from(a: AttestationV6) -> Self {
        keel_attest::Attestation {
            chain: a.chain,
            tx_hash: a.tx_hash,
            index: a.index,
            deposit_index: a.deposit_index,
            amount: a.amount,
            asset: a.asset,
            external_height: a.external_height,
            external_tx: None,
        }
    }
}

#[derive(BorshDeserialize)]
pub struct PendingDepositV6 {
    pub attestation: AttestationV6,
    pub quorum: keel_attest::Quorum,
    pub first_height: u64,
    pub last_height: u64,
    pub status: crate::modules::vaults::DepositStatus,
}

impl From<PendingDepositV6> for crate::modules::vaults::PendingDeposit {
    fn from(p: PendingDepositV6) -> Self {
        crate::modules::vaults::PendingDeposit {
            attestation: p.attestation.into(),
            quorum: p.quorum,
            first_height: p.first_height,
            last_height: p.last_height,
            status: p.status,
        }
    }
}

#[derive(BorshDeserialize)]
pub struct OutboundV6 {
    pub id: keel_actions::OutboundId,
    pub owner: keel_types::Address,
    pub asset: keel_types::Asset,
    pub chain: keel_actions::Chain,
    pub to: String,
    pub amount: keel_types::Amount,
    pub fee_asset: keel_types::Asset,
    pub fee_estimate: keel_types::Amount,
    pub status: crate::modules::vaults::OutboundStatus,
    pub batch_id: Option<u64>,
    pub created_height: u64,
    pub quorum: keel_attest::Quorum,
    pub tx_hash: Option<[u8; 32]>,
    pub votes: Vec<(keel_types::Address, keel_types::Amount, bool)>,
}

impl From<OutboundV6> for crate::modules::vaults::Outbound {
    fn from(o: OutboundV6) -> Self {
        crate::modules::vaults::Outbound {
            id: o.id,
            owner: o.owner,
            asset: o.asset,
            chain: o.chain,
            to: o.to,
            amount: o.amount,
            fee_asset: o.fee_asset,
            fee_estimate: o.fee_estimate,
            status: o.status,
            batch_id: o.batch_id,
            created_height: o.created_height,
            quorum: o.quorum,
            tx_hash: o.tx_hash,
            votes: o.votes,
            external_tx: None,
        }
    }
}

#[derive(BorshDeserialize)]
pub struct VaultsStateV6 {
    pub vaults:
        std::collections::BTreeMap<(keel_actions::Chain, u64), crate::modules::vaults::Vault>,
    pub next_deposit_index: std::collections::BTreeMap<keel_actions::Chain, u64>,
    pub deposit_owner: std::collections::BTreeMap<(keel_actions::Chain, u64), keel_types::Address>,
    pub deposit_index_of:
        std::collections::BTreeMap<(keel_types::Address, keel_actions::Chain), u64>,
    pub pending: std::collections::BTreeMap<[u8; 32], PendingDepositV6>,
    pub credited: std::collections::BTreeSet<(keel_actions::Chain, [u8; 32], u32)>,
    pub held: Vec<crate::modules::vaults::HeldDeposit>,
    pub btc_chain: Option<keel_lc_btc::HeaderChain>,
    pub eth_sync: Option<keel_lc_eth::SyncCommitteeState>,
    pub token_contracts: std::collections::BTreeMap<keel_types::Asset, Vec<u8>>,
    pub outbounds: std::collections::BTreeMap<keel_actions::OutboundId, OutboundV6>,
    pub next_outbound_id: u64,
    pub batches: std::collections::BTreeMap<u64, crate::modules::vaults::Batch>,
    pub next_batch_id: u64,
    pub halted: std::collections::BTreeSet<keel_types::Asset>,
    pub fee_reports: std::collections::BTreeMap<
        keel_actions::Chain,
        std::collections::BTreeMap<keel_types::Address, u64>,
    >,
    pub credited_today: std::collections::BTreeMap<keel_actions::Chain, (u64, keel_types::Amount)>,
}

impl From<VaultsStateV6> for crate::modules::vaults::VaultsState {
    fn from(v: VaultsStateV6) -> Self {
        crate::modules::vaults::VaultsState {
            vaults: v.vaults,
            next_deposit_index: v.next_deposit_index,
            deposit_owner: v.deposit_owner,
            deposit_index_of: v.deposit_index_of,
            pending: v.pending.into_iter().map(|(k, p)| (k, p.into())).collect(),
            credited: v.credited,
            held: v.held,
            btc_chain: v.btc_chain,
            eth_sync: v.eth_sync,
            token_contracts: v.token_contracts,
            outbounds: v
                .outbounds
                .into_iter()
                .map(|(k, o)| (k, o.into()))
                .collect(),
            next_outbound_id: v.next_outbound_id,
            batches: v.batches,
            next_batch_id: v.next_batch_id,
            halted: v.halted,
            fee_reports: v.fee_reports,
            credited_today: v.credited_today,
        }
    }
}

#[derive(BorshDeserialize)]
pub struct CustodyStateV6 {
    pub vaults: std::collections::BTreeMap<
        (keel_actions::Chain, keel_types::Address),
        crate::modules::custody::CustodyVault,
    >,
    pub next_deposit_index:
        std::collections::BTreeMap<(keel_actions::Chain, keel_types::Address), u64>,
    pub deposit_owner: std::collections::BTreeMap<
        (keel_actions::Chain, keel_types::Address, u64),
        keel_types::Address,
    >,
    pub deposit_index_of:
        std::collections::BTreeMap<(keel_types::Address, keel_actions::Chain), u64>,
    pub custodian_of: std::collections::BTreeMap<keel_types::Address, keel_types::Address>,
    pub pending: std::collections::BTreeMap<[u8; 32], PendingDepositV6>,
    pub credited: std::collections::BTreeSet<(keel_actions::Chain, [u8; 32], u32)>,
    pub outbound_vault: std::collections::BTreeMap<keel_actions::OutboundId, keel_types::Address>,
    pub batch_vault: std::collections::BTreeMap<u64, keel_types::Address>,
    pub liabilities:
        std::collections::BTreeMap<(keel_types::Address, keel_types::Asset), keel_types::Amount>,
    pub halted: std::collections::BTreeSet<(keel_types::Address, keel_types::Asset)>,
}

impl From<CustodyStateV6> for crate::modules::custody::CustodyState {
    fn from(c: CustodyStateV6) -> Self {
        crate::modules::custody::CustodyState {
            vaults: c.vaults,
            next_deposit_index: c.next_deposit_index,
            deposit_owner: c.deposit_owner,
            deposit_index_of: c.deposit_index_of,
            custodian_of: c.custodian_of,
            pending: c.pending.into_iter().map(|(k, p)| (k, p.into())).collect(),
            credited: c.credited,
            outbound_vault: c.outbound_vault,
            batch_vault: c.batch_vault,
            liabilities: c.liabilities,
            halted: c.halted,
        }
    }
}

/// Schema 7: everything of the current layout but `recovery`.
#[derive(BorshDeserialize)]
pub struct StateV7 {
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
    pub custody: crate::modules::custody::CustodyState,
    pub arbitration: crate::modules::arbitration::ArbitrationState,
    pub pools: crate::modules::pools::PoolsState,
    pub liquidity: crate::modules::liquidity::LiquidityState,
}

impl From<StateV7> for State {
    fn from(v: StateV7) -> Self {
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
            custody: v.custody,
            arbitration: v.arbitration,
            pools: v.pools,
            liquidity: v.liquidity,
            recovery: Default::default(),
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
