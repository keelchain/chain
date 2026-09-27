//! Governance: proposals, KEEL-weighted voting, tally, timelock, execution.
//! Every knob the backoffice used to have is a `ParamChange`; listing,
//! treasury spend, upgrades and set membership are their own kinds.
//!
//! Lifecycle: `Propose` locks `params.proposal_deposit` KEEL into the
//! proposer's restricted `proposal_deposit` account. Votes carry the
//! voter's bonded weight at vote time (re-voting replaces the earlier
//! vote). At `voting_end` the tally runs in `end_block`:
//!   - quorum: total voted weight >= quorum_bps of total bonded, else Rejected;
//!   - veto: veto weight >= veto_bps of votes -> Vetoed, deposit burned;
//!   - passed: yes >= threshold_bps of (yes + no), else Rejected.
//!
//! Deposits are refunded on Passed and Rejected. Anyone may `ExecuteProposal`
//! after `timelock_end`; execution success/failure is recorded, never
//! retried.
//!
//! Devnet only (chain_id 1): `ParamChange { key: "house_operator_seed" }`
//! sets `state.gov_house_operator` to the keypair-from-seed address, and
//! `"house_operator_tag"` to `Address::tagged(value)`, so a devnet can
//! nominate the house market maker without a dedicated action.

use crate::{
    context::BlockContext,
    modules::vaults,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, Chain, Proposal, ProposalKind, VoteChoice};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, Address, Amount};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{markets, stable, staking, tokens};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum ProposalStatus {
    Voting,
    Passed,
    Rejected,
    Vetoed,
    Executed,
    Failed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalStatus::Voting => "voting",
            ProposalStatus::Passed => "passed",
            ProposalStatus::Rejected => "rejected",
            ProposalStatus::Vetoed => "vetoed",
            ProposalStatus::Executed => "executed",
            ProposalStatus::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ProposalRecord {
    pub id: u64,
    pub proposer: Address,
    pub title: String,
    pub description: String,
    pub kind: ProposalKind,
    pub deposit: Amount,
    pub submit_height: u64,
    pub voting_end: u64,
    pub timelock_end: u64,
    pub yes: Amount,
    pub no: Amount,
    pub abstain: Amount,
    pub veto: Amount,
    pub status: ProposalStatus,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct GovState {
    pub proposals: BTreeMap<u64, ProposalRecord>,
    /// proposal -> voter -> (choice, weight at vote time)
    pub votes: BTreeMap<u64, BTreeMap<Address, (VoteChoice, Amount)>>,
    pub next_id: u64,
    /// Executed software upgrades: (version, activation height).
    pub upgrades: Vec<(String, u64)>,
    /// Address allowed to change parameters directly with `SetParam`
    /// (the platform super admin at launch; governance can revoke it).
    pub param_admin: Option<Address>,
}

/// Direct parameter change by the appointed admin. Same validation as an
/// executed `ParamChange`, without the proposal round trip.
pub fn set_param(
    state: &mut State,
    signer: Address,
    key: &str,
    value: u128,
) -> Result<Vec<Event>, VmError> {
    if state.gov.param_admin != Some(signer) {
        return Err(VmError::Unauthorized);
    }
    if key.starts_with("clients.") {
        return super::clients::set_param(state, key, value);
    }
    let mut next = state.params.clone();
    if !next.set(key, value) {
        return Err(VmError::Invalid(format!("cannot set {key}")));
    }
    if !next.fee_split_ok() {
        return Err(VmError::Invalid("fee split must sum to 10000".into()));
    }
    if !next.block_timing_ok() {
        return Err(VmError::Invalid("block intervals must satisfy 0 <= min_block_interval_ms <= idle_block_interval_ms <= 60000".into()));
    }
    state.params = next;
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

fn deposit_key(owner: Address, native: &keel_types::Asset) -> AccountKey {
    AccountKey::new(owner, native.clone(), "proposal_deposit").expect("catalog type")
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::Propose(p) => propose(state, ctx, signer, tx_id, p),
        Action::Vote {
            proposal_id,
            choice,
        } => vote(state, signer, *proposal_id, *choice),
        Action::ExecuteProposal { proposal_id } => execute(state, ctx, *proposal_id),
        _ => Err(VmError::Invalid("not a governance action".into())),
    }
}

fn validate_kind(state: &State, kind: &ProposalKind) -> Result<(), VmError> {
    match kind {
        ProposalKind::ParamChange { key, .. } => {
            if state.params.get(key).is_none() && !is_devnet_operator_key(state, key) {
                return Err(VmError::Invalid(format!("unknown param {key}")));
            }
        }
        ProposalKind::ListPair(cfg) => {
            if !state.tokens.is_registered(&cfg.base_asset)
                || !state.tokens.is_registered(&cfg.quote_asset)
            {
                return Err(VmError::Invalid("pair assets must be registered".into()));
            }
            if cfg.symbol.is_empty() || cfg.lot_size == 0 || cfg.tick_size == 0 {
                return Err(VmError::Invalid("bad pair config".into()));
            }
        }
        ProposalKind::RegisterAsset { asset, decimals } => {
            if state.tokens.is_registered(asset) {
                return Err(VmError::Invalid("asset already registered".into()));
            }
            if *decimals > 30 {
                return Err(VmError::Invalid("decimals too large".into()));
            }
        }
        ProposalKind::TreasurySpend { asset, amount, .. } => {
            if !state.tokens.is_registered(asset) || *amount == 0 {
                return Err(VmError::Invalid("bad treasury spend".into()));
            }
        }
        ProposalKind::SoftwareUpgrade { version, height } => {
            if version.is_empty() || *height <= state.height {
                return Err(VmError::Invalid(
                    "upgrade height must be in the future".into(),
                ));
            }
        }
        ProposalKind::SetObservers { members, threshold } => {
            if members.is_empty() || *threshold == 0 || *threshold as usize > members.len() {
                return Err(VmError::Invalid("bad observer set".into()));
            }
        }
        ProposalKind::SetStableBasket { asset, .. }
            if !state.tokens.is_registered(asset) || *asset == state.tokens.stable =>
        {
            return Err(VmError::Invalid("bad basket asset".into()));
        }
        _ => {}
    }
    Ok(())
}

fn is_devnet_operator_key(state: &State, key: &str) -> bool {
    state.chain_id == 1 && (key == "house_operator_seed" || key == "house_operator_tag")
}

fn propose(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    p: &Proposal,
) -> Result<Vec<Event>, VmError> {
    if p.title.is_empty() || p.title.len() > 200 || p.description.len() > 10_000 {
        return Err(VmError::Invalid("bad title/description length".into()));
    }
    validate_kind(state, &p.kind)?;
    let native = state.tokens.native.clone();
    let deposit = state.params.proposal_deposit;
    if deposit > 0 {
        state.ledger.post(
            &format!("proposal:{}", tokens::hex(tx_id)),
            TxType::ProposalDeposit,
            None,
            None,
            vec![
                Record::debit(tokens::deposit_key(signer, &native), deposit),
                Record::credit(deposit_key(signer, &native), deposit),
            ],
        )?;
    }
    let id = state.gov.next_id;
    state.gov.next_id += 1;
    let voting_end = ctx.height.saturating_add(state.params.voting_period_blocks);
    let timelock_end = voting_end.saturating_add(state.params.timelock_blocks);
    state.gov.proposals.insert(
        id,
        ProposalRecord {
            id,
            proposer: signer,
            title: p.title.clone(),
            description: p.description.clone(),
            kind: p.kind.clone(),
            deposit,
            submit_height: ctx.height,
            voting_end,
            timelock_end,
            yes: 0,
            no: 0,
            abstain: 0,
            veto: 0,
            status: ProposalStatus::Voting,
        },
    );
    Ok(vec![Event::ProposalCreated {
        proposal_id: id,
        proposer: signer,
    }])
}

fn add_tally(p: &mut ProposalRecord, choice: VoteChoice, weight: Amount, sign: bool) {
    let slot = match choice {
        VoteChoice::Yes => &mut p.yes,
        VoteChoice::No => &mut p.no,
        VoteChoice::Abstain => &mut p.abstain,
        VoteChoice::Veto => &mut p.veto,
    };
    *slot = if sign {
        slot.saturating_add(weight)
    } else {
        slot.saturating_sub(weight)
    };
}

fn vote(
    state: &mut State,
    signer: Address,
    proposal_id: u64,
    choice: VoteChoice,
) -> Result<Vec<Event>, VmError> {
    let weight = staking::voting_weight(state, &signer);
    if weight == 0 {
        return Err(VmError::Invalid("no bonded KEEL to vote with".into()));
    }
    let p = state
        .gov
        .proposals
        .get_mut(&proposal_id)
        .ok_or_else(|| VmError::NotFound(format!("proposal {proposal_id}")))?;
    if p.status != ProposalStatus::Voting {
        return Err(VmError::Invalid("voting is closed".into()));
    }
    let votes = state.gov.votes.entry(proposal_id).or_default();
    if let Some((old_choice, old_weight)) = votes.insert(signer, (choice, weight)) {
        add_tally(p, old_choice, old_weight, false);
    }
    add_tally(p, choice, weight, true);
    Ok(vec![Event::Voted {
        proposal_id,
        voter: signer,
        weight,
    }])
}

fn execute(state: &mut State, ctx: &BlockContext, proposal_id: u64) -> Result<Vec<Event>, VmError> {
    let p = state
        .gov
        .proposals
        .get(&proposal_id)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("proposal {proposal_id}")))?;
    if p.status != ProposalStatus::Passed {
        return Err(VmError::Invalid(format!(
            "proposal is {}",
            p.status.as_str()
        )));
    }
    if ctx.height < p.timelock_end {
        return Err(VmError::Invalid("timelock has not elapsed".into()));
    }
    let mut events = Vec::new();
    let sp = state.ledger.savepoint();
    let result = execute_kind(state, ctx, &p.kind, &mut events);
    let ok = result.is_ok();
    if !ok {
        state.ledger.rollback(sp);
        events.clear();
    }
    let rec = state.gov.proposals.get_mut(&proposal_id).expect("exists");
    rec.status = if ok {
        ProposalStatus::Executed
    } else {
        ProposalStatus::Failed
    };
    events.push(Event::ProposalExecuted { proposal_id, ok });
    Ok(events)
}

fn execute_kind(
    state: &mut State,
    ctx: &BlockContext,
    kind: &ProposalKind,
    events: &mut Vec<Event>,
) -> Result<(), VmError> {
    match kind {
        ProposalKind::ParamChange { key, value } => {
            if is_devnet_operator_key(state, key) {
                let addr = if key == "house_operator_seed" {
                    keel_crypto::Keypair::from_seed(
                        u64::try_from(*value)
                            .map_err(|_| VmError::Invalid("seed too large".into()))?,
                    )
                    .address()
                } else {
                    Address::tagged(
                        u64::try_from(*value)
                            .map_err(|_| VmError::Invalid("tag too large".into()))?,
                    )
                };
                state.gov_house_operator = Some(addr);
                events.push(Event::ParamChanged {
                    key: key.clone(),
                    value: *value,
                });
                return Ok(());
            }
            let mut next = state.params.clone();
            if !next.set(key, *value) {
                return Err(VmError::Invalid(format!("cannot set {key}")));
            }
            if !next.fee_split_ok() {
                return Err(VmError::Invalid("fee split must sum to 10000".into()));
            }
            state.params = next;
            events.push(Event::ParamChanged {
                key: key.clone(),
                value: *value,
            });
        }
        ProposalKind::ListPair(cfg) => markets::list_pair(state, cfg.clone()),
        ProposalKind::DelistPair { symbol } => markets::delist_pair(state, symbol),
        ProposalKind::RegisterAsset { asset, decimals } => {
            if state.tokens.is_registered(asset) {
                return Err(VmError::Invalid("asset already registered".into()));
            }
            let kind = match asset.chain().and_then(Chain::parse) {
                Some(chain) => tokens::AssetKind::Vault { chain },
                None => tokens::AssetKind::Native,
            };
            if matches!(kind, tokens::AssetKind::Native) && asset.chain().is_some() {
                return Err(VmError::Invalid("unknown chain prefix".into()));
            }
            tokens::register(state, asset.clone(), *decimals, kind);
        }
        ProposalKind::TreasurySpend { to, asset, amount } => {
            if to.is_system() {
                return Err(VmError::Invalid("recipient must be a user".into()));
            }
            state.ledger.post(
                &format!("treasury:{}:{}", ctx.height, state.ledger.next_seq()),
                TxType::TreasuryPayout,
                None,
                None,
                vec![
                    Record::debit(tokens::system_key(asset, "treasury"), *amount),
                    Record::credit(tokens::deposit_key(*to, asset), *amount),
                ],
            )?;
            // Dual-control invariant: the platform must still cover every user
            // liability in a vault asset after the payout.
            if matches!(
                state.tokens.kind(asset),
                Some(tokens::AssetKind::Vault { .. })
            ) {
                let reserves = state.ledger.system_reserves(asset).max(0) as Amount;
                if reserves < state.ledger.user_liabilities(asset) {
                    return Err(VmError::Invalid(
                        "payout would breach reserves >= liabilities".into(),
                    ));
                }
            }
        }
        ProposalKind::SoftwareUpgrade { version, height } => {
            state.gov.upgrades.push((version.clone(), *height));
        }
        ProposalKind::SetArbitrators { members } => staking::set_arbitrators(state, members),
        ProposalKind::SetAttesters { members } => {
            state.attest.attesters = members.iter().copied().collect();
        }
        ProposalKind::SetObservers { members, threshold } => {
            staking::set_observers(state, members, *threshold)
        }
        ProposalKind::SetStableBasket {
            asset,
            cap,
            enabled,
        } => stable::set_basket(state, asset.clone(), *cap, *enabled),
        ProposalKind::SetBtcCheckpoint(cp) => vaults::apply_btc_checkpoint(state, cp)?,
        ProposalKind::SetEthCheckpoint(cp) => vaults::apply_eth_checkpoint(state, cp),
        ProposalKind::SetParamAdmin { admin } => {
            state.gov.param_admin = *admin;
            events.push(Event::ParamAdminChanged { admin: *admin });
        }
        ProposalKind::SetTokenContract { asset, contract } => {
            if contract.len() != 20 || !state.tokens.is_registered(asset) {
                return Err(VmError::Invalid(
                    "token contract must be 20 bytes for a registered asset".into(),
                ));
            }
            vaults::set_token_contract(state, asset.clone(), contract.clone());
        }
        ProposalKind::PauseModule {
            module,
            until_height,
        } => {
            state.paused.insert(module.clone(), *until_height);
        }
        ProposalKind::Text => {}
    }
    Ok(())
}

fn settle_deposit(state: &mut State, p: &ProposalRecord, burn: bool) {
    if p.deposit == 0 {
        return;
    }
    let native = state.tokens.native.clone();
    let (ext, tx_type, to) = if burn {
        (
            format!("proposal:{}:burn", p.id),
            TxType::Burn,
            tokens::system_key(&native, "burn"),
        )
    } else {
        (
            format!("proposal:{}:refund", p.id),
            TxType::ProposalRefund,
            tokens::deposit_key(p.proposer, &native),
        )
    };
    let _ = state.ledger.post(
        &ext,
        tx_type,
        None,
        None,
        vec![
            Record::debit(deposit_key(p.proposer, &native), p.deposit),
            Record::credit(to, p.deposit),
        ],
    );
}

pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let due: Vec<u64> = state
        .gov
        .proposals
        .values()
        .filter(|p| p.status == ProposalStatus::Voting && p.voting_end <= ctx.height)
        .map(|p| p.id)
        .collect();
    for id in due {
        let p = state.gov.proposals[&id].clone();
        let total_votes = p
            .yes
            .saturating_add(p.no)
            .saturating_add(p.abstain)
            .saturating_add(p.veto);
        let bonded = staking::total_bonded(state);
        let quorum = mul_div_floor(bonded, state.params.quorum_bps as Amount, 10_000).unwrap_or(0);
        let veto_line =
            mul_div_floor(total_votes, state.params.veto_bps as Amount, 10_000).unwrap_or(0);
        let decided = p.yes.saturating_add(p.no);
        let pass_line =
            mul_div_floor(decided, state.params.threshold_bps as Amount, 10_000).unwrap_or(0);
        let status = if total_votes == 0 || total_votes < quorum {
            ProposalStatus::Rejected
        } else if p.veto > 0 && p.veto >= veto_line {
            ProposalStatus::Vetoed
        } else if p.yes > 0 && p.yes >= pass_line {
            ProposalStatus::Passed
        } else {
            ProposalStatus::Rejected
        };
        settle_deposit(state, &p, status == ProposalStatus::Vetoed);
        state.gov.proposals.get_mut(&id).expect("exists").status = status;
        events.push(Event::ProposalTallied {
            proposal_id: id,
            status: status.as_str().into(),
        });
    }
    events
}
