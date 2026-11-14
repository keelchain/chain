//! Self-service recovery of coins that landed at a deposit address but
//! can never be credited (2026-11-14): a token the chain does not list,
//! an EVM network the chain does not watch, the wrong cluster. The chain
//! already knows who owns every deposit index, so no one has to decide
//! anything: the owner (or, for a custody vault, its custodian) asks, the
//! observers move whatever sits at the address to the destination the
//! owner named, and report back. The signing policy accepts such a move
//! only against an open recovery row, as a payout only against an
//! outbound row.
//!
//! What can be recovered is bounded by the keys: anything an EVM address
//! holds on any EVM chain (the family shares one key), anything at a
//! Solana address on either cluster, an unlisted TRC-20 at a Tron
//! address. Nothing can land at a Bitcoin address on another chain, so
//! Bitcoin has no recovery. Gas comes from the address itself (the owner
//! sends a little native coin to it first) or, on a chain the observers
//! serve, from the hot address.
//!
//! The requester pays `recovery.fee_keel`; a row nobody can serve expires
//! after `recovery.expiry_blocks`.

use crate::{
    context::BlockContext,
    modules::{clients, fees, staking, tokens, vaults},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, Chain, RecoveryObservation, RecoveryRequest};
use keel_attest::Quorum;
use keel_ledger::TxType;
use keel_types::{Address, Amount};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct RecoveryParams {
    /// Charged to the requester, in KEEL smallest units (0 = free).
    pub fee_keel: u128,
    /// Blocks an open row waits for the observers before it expires.
    pub expiry_blocks: u64,
}

impl Default for RecoveryParams {
    fn default() -> Self {
        Self {
            fee_keel: 0,
            expiry_blocks: 20_000,
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum RecoveryStatus {
    Open,
    Done,
    Failed,
    Expired,
}

impl RecoveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryStatus::Open => "Open",
            RecoveryStatus::Done => "Done",
            RecoveryStatus::Failed => "Failed",
            RecoveryStatus::Expired => "Expired",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Recovery {
    pub id: u64,
    /// The address family (the vault whose child the address is).
    pub chain: Chain,
    /// An EVM chain id when the coins sit on an EVM network other than
    /// `chain` itself; `None` = `chain`.
    pub evm_chain_id: Option<u64>,
    /// Token contract / mint; `None` = the native coin.
    pub token: Option<String>,
    pub to: String,
    pub owner: Address,
    /// The custody vault the index belongs to; `None` = the network vault.
    pub custodian: Option<Address>,
    pub deposit_index: u64,
    pub created_height: u64,
    pub status: RecoveryStatus,
    pub quorum: Quorum,
    /// (observer, amount moved, success) per vote.
    pub votes: Vec<(Address, Amount, bool)>,
    pub tx_hash: Option<[u8; 32]>,
    pub external_tx: Option<String>,
    /// What the quorum settled on once closed.
    pub amount: Amount,
    pub closed_height: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct RecoveryState {
    pub params: RecoveryParams,
    pub next_id: u64,
    pub rows: BTreeMap<u64, Recovery>,
}

pub const PARAM_KEYS: &[&str] = &["recovery.fee_keel", "recovery.expiry_blocks"];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let p = &state.recovery.params;
    match key {
        "recovery.fee_keel" => Some(p.fee_keel),
        "recovery.expiry_blocks" => Some(p.expiry_blocks as u128),
        _ => None,
    }
}

pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    let p = &mut state.recovery.params;
    match key {
        "recovery.fee_keel" => p.fee_keel = value,
        "recovery.expiry_blocks" => {
            let v = u64::try_from(value).map_err(|_| VmError::Invalid("too large".into()))?;
            if v == 0 {
                return Err(VmError::Invalid(
                    "recovery.expiry_blocks must be > 0".into(),
                ));
            }
            p.expiry_blocks = v;
        }
        _ => return Err(VmError::Invalid(format!("unknown parameter {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::RequestRecovery(r) => request(state, ctx, signer, tx_id, r),
        Action::ObserveRecovery(o) => observe(state, ctx, signer, o),
        _ => Err(VmError::Invalid("not a recovery action".into())),
    }
}

fn b58(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() && !b"0OIl".contains(&c))
}

/// Shape check of a token id on the family's chains.
fn valid_token(chain: Chain, token: &str) -> bool {
    match chain {
        Chain::Ethereum | Chain::Bsc => {
            token.len() == 42
                && token.starts_with("0x")
                && token[2..].bytes().all(|c| c.is_ascii_hexdigit())
        }
        Chain::Tron => token.len() == 34 && token.starts_with('T') && b58(token),
        Chain::Solana => (32..=44).contains(&token.len()) && b58(token),
        Chain::Bitcoin => false,
    }
}

fn request(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    r: &RecoveryRequest,
) -> Result<Vec<Event>, VmError> {
    if r.chain == Chain::Bitcoin {
        return Err(VmError::Invalid(
            "nothing can land at a Bitcoin address on another chain".into(),
        ));
    }
    if r.evm_chain_id.is_some() && !r.chain.is_evm() {
        return Err(VmError::Invalid("evm_chain_id needs an EVM chain".into()));
    }
    if !vaults::valid_destination(r.chain, &r.to) {
        return Err(VmError::Invalid("bad destination address".into()));
    }
    if let Some(t) = &r.token {
        if !valid_token(r.chain, t) {
            return Err(VmError::Invalid("bad token id".into()));
        }
    }
    // Whose index. The network vault's index is the signer's own; a
    // custody vault's belongs to `owner`, asked for by the custodian (or
    // a client member) that backs that account.
    let (owner, custodian, deposit_index) = match r.custodian {
        None => {
            let index = state.vaults.deposit_index(signer, r.chain).ok_or_else(|| {
                VmError::NotFound(format!(
                    "no {} deposit address for this account",
                    r.chain.as_str()
                ))
            })?;
            (signer, None, index)
        }
        Some(c) => {
            let owner = r
                .owner
                .ok_or_else(|| VmError::Invalid("owner is required with custodian".into()))?;
            if signer != c && clients::client_of(state, &signer) != Some(c) {
                return Err(VmError::Unauthorized);
            }
            if state.custody.custodian_of.get(&owner) != Some(&c) {
                return Err(VmError::Invalid(
                    "account is not in that client's custody".into(),
                ));
            }
            let index = *state
                .custody
                .deposit_index_of
                .get(&(owner, r.chain))
                .ok_or_else(|| {
                    VmError::NotFound(format!(
                        "no {} custody address for that account",
                        r.chain.as_str()
                    ))
                })?;
            (owner, Some(c), index)
        }
    };
    let duplicate = state.recovery.rows.values().any(|x| {
        x.status == RecoveryStatus::Open
            && x.chain == r.chain
            && x.evm_chain_id == r.evm_chain_id
            && x.token == r.token
            && x.custodian == custodian
            && x.deposit_index == deposit_index
    });
    if duplicate {
        return Err(VmError::Invalid(
            "a recovery of that asset from that address is already open".into(),
        ));
    }
    let fee = state.recovery.params.fee_keel;
    if fee > 0 {
        let native = state.tokens.native.clone();
        fees::collect(
            state,
            &format!("recovery:fee:{}", tokens::hex(tx_id)),
            TxType::BudgetPurchase,
            None,
            tokens::deposit_key(signer, &native),
            &native,
            fee,
        )
        .map_err(|_| VmError::Invalid("cannot pay the recovery fee".into()))?;
    }
    let id = state.recovery.next_id.max(1);
    state.recovery.next_id = id + 1;
    state.recovery.rows.insert(
        id,
        Recovery {
            id,
            chain: r.chain,
            evm_chain_id: r.evm_chain_id,
            token: r.token.clone(),
            to: r.to.clone(),
            owner,
            custodian,
            deposit_index,
            created_height: ctx.height,
            status: RecoveryStatus::Open,
            quorum: Quorum::default(),
            votes: Vec::new(),
            tx_hash: None,
            external_tx: None,
            amount: 0,
            closed_height: None,
        },
    );
    Ok(vec![Event::RecoveryRequested {
        id,
        owner,
        custodian,
        chain: r.chain.as_str().into(),
        evm_chain_id: r.evm_chain_id,
        token: r.token.clone(),
        to: r.to.clone(),
        deposit_index,
        fee,
    }])
}

fn observe(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    o: &RecoveryObservation,
) -> Result<Vec<Event>, VmError> {
    if !staking::is_observer(state, &signer) {
        return Err(VmError::Unauthorized);
    }
    let (observers, threshold) = staking::observers(state);
    let row = state
        .recovery
        .rows
        .get_mut(&o.id)
        .ok_or_else(|| VmError::NotFound(format!("recovery {}", o.id)))?;
    if row.status != RecoveryStatus::Open {
        return Err(VmError::Invalid("recovery is not open".into()));
    }
    if !row.quorum.vote(signer) {
        return Err(VmError::Invalid("observer already voted".into()));
    }
    row.votes.push((signer, o.amount, o.success));
    if row.tx_hash.is_none() {
        row.tx_hash = Some(o.tx_hash);
        row.external_tx = o.external_tx.clone();
    }
    if !row.quorum.reached(&observers, threshold) {
        return Ok(vec![Event::RecoveryObserved {
            id: o.id,
            votes: row.quorum.count_within(&observers),
        }]);
    }
    let mut amounts: Vec<Amount> = row.votes.iter().map(|(_, a, _)| *a).collect();
    amounts.sort_unstable();
    let amount = amounts.get(amounts.len() / 2).copied().unwrap_or(o.amount);
    let successes = row.votes.iter().filter(|(_, _, ok)| *ok).count();
    let success = successes * 2 >= row.votes.len();
    row.amount = amount;
    row.closed_height = Some(ctx.height);
    row.status = if success {
        RecoveryStatus::Done
    } else {
        RecoveryStatus::Failed
    };
    let tx_hash = row.tx_hash.map(|h| tokens::hex(&h)).unwrap_or_default();
    let external_tx = row.external_tx.clone();
    Ok(vec![if success {
        Event::RecoveryDone {
            id: o.id,
            amount,
            tx_hash,
            external_tx,
        }
    } else {
        Event::RecoveryFailed { id: o.id, tx_hash }
    }])
}

/// Expire open rows nobody served, and forget closed rows after another
/// expiry period so the map stays bounded.
pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let expiry = state.recovery.params.expiry_blocks.max(1);
    let mut events = Vec::new();
    let mut drop = Vec::new();
    for (id, row) in state.recovery.rows.iter_mut() {
        match row.status {
            RecoveryStatus::Open if ctx.height >= row.created_height.saturating_add(expiry) => {
                row.status = RecoveryStatus::Expired;
                row.closed_height = Some(ctx.height);
                events.push(Event::RecoveryExpired { id: *id });
            }
            RecoveryStatus::Open => {}
            _ => {
                if row
                    .closed_height
                    .is_some_and(|h| ctx.height >= h.saturating_add(expiry))
                {
                    drop.push(*id);
                }
            }
        }
    }
    for id in drop {
        state.recovery.rows.remove(&id);
    }
    events
}
