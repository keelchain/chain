//! Disputes over P2P trades (docs/plan.md §8). Either party opens
//! one, both submit evidence hashes, a bonded arbitrator rules, and the
//! escrow is paid out in one atomic sequence. The dispute fee comes out of
//! the losing side's share (capped at that share, so a side with nothing
//! coming pays nothing).

use crate::{
    context::BlockContext,
    modules::p2p::{finish_trade, TradeStatus},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, Ruling, TradeId};
use keel_crypto::Hash32;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{fees, staking, tokens};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Evidence {
    pub by: Address,
    pub hash: Hash32,
    pub height: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Dispute {
    pub trade_id: TradeId,
    pub opened_by: Address,
    pub opened_at: u64,
    pub evidence: Vec<Evidence>,
    pub ruling: Option<Ruling>,
    pub ruled_by: Option<Address>,
    pub ruled_at: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct DisputesState {
    pub disputes: BTreeMap<TradeId, Dispute>,
}

pub const MAX_EVIDENCE: usize = 32;

fn escrow_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "marketplace_escrow").expect("catalog type")
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    _tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::OpenDispute {
            trade_id,
            evidence_hash,
        } => open(state, ctx, signer, *trade_id, *evidence_hash),
        Action::SubmitEvidence {
            trade_id,
            evidence_hash,
        } => submit(state, ctx, signer, *trade_id, *evidence_hash),
        Action::RuleDispute { trade_id, ruling } => rule(state, ctx, signer, *trade_id, *ruling),
        _ => Err(VmError::Invalid("not a disputes action".into())),
    }
}

fn open(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    trade_id: TradeId,
    evidence_hash: Hash32,
) -> Result<Vec<Event>, VmError> {
    let t = state
        .p2p
        .trades
        .get(&trade_id)
        .ok_or_else(|| VmError::NotFound(format!("trade {trade_id}")))?;
    if signer != t.buyer && signer != t.seller {
        return Err(VmError::Unauthorized);
    }
    if t.status != TradeStatus::Paid {
        return Err(VmError::Invalid("only a paid trade can be disputed".into()));
    }
    let now = ctx.seconds();
    if signer == t.buyer {
        let paid_at = t.paid_at.unwrap_or(now);
        if now < paid_at.saturating_add(state.params.release_grace_secs as u64) {
            return Err(VmError::Invalid(
                "release grace period still running".into(),
            ));
        }
    }
    let t = state.p2p.trades.get_mut(&trade_id).expect("checked");
    t.status = TradeStatus::Disputed;
    state.disputes.disputes.insert(
        trade_id,
        Dispute {
            trade_id,
            opened_by: signer,
            opened_at: now,
            evidence: vec![Evidence {
                by: signer,
                hash: evidence_hash,
                height: ctx.height,
            }],
            ruling: None,
            ruled_by: None,
            ruled_at: None,
        },
    );
    Ok(vec![Event::DisputeOpened {
        trade_id,
        by: signer,
    }])
}

fn submit(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    trade_id: TradeId,
    evidence_hash: Hash32,
) -> Result<Vec<Event>, VmError> {
    let t = state
        .p2p
        .trades
        .get(&trade_id)
        .ok_or_else(|| VmError::NotFound(format!("trade {trade_id}")))?;
    if signer != t.buyer && signer != t.seller {
        return Err(VmError::Unauthorized);
    }
    let d = state
        .disputes
        .disputes
        .get_mut(&trade_id)
        .ok_or_else(|| VmError::NotFound("dispute".into()))?;
    if d.ruling.is_some() {
        return Err(VmError::Invalid("dispute already ruled".into()));
    }
    if d.evidence.len() >= MAX_EVIDENCE {
        return Err(VmError::Invalid("too much evidence".into()));
    }
    d.evidence.push(Evidence {
        by: signer,
        hash: evidence_hash,
        height: ctx.height,
    });
    Ok(Vec::new())
}

/// Payout shares of `amount` (the buyer's principal) for a ruling.
pub fn shares(amount: Amount, ruling: Ruling) -> (Amount, Amount) {
    match ruling {
        Ruling::WinsSeller => (0, amount),
        Ruling::WinsBuyer => (amount, 0),
        Ruling::Split { buyer_bps } => {
            let b = mul_div_floor(amount, buyer_bps.min(10_000) as Amount, 10_000).unwrap_or(0);
            (b, amount.saturating_sub(b))
        }
    }
}

fn rule(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    trade_id: TradeId,
    ruling: Ruling,
) -> Result<Vec<Event>, VmError> {
    // Any bonded arbitrator may rule; the ruling window in params only
    // matters once arbitrators are assigned per dispute (kept simple).
    if !staking::is_arbitrator(state, &signer) {
        return Err(VmError::Unauthorized);
    }
    if let Ruling::Split { buyer_bps } = ruling {
        if buyer_bps > 10_000 {
            return Err(VmError::Invalid("buyer_bps above 10000".into()));
        }
    }
    let t = state
        .p2p
        .trades
        .get(&trade_id)
        .ok_or_else(|| VmError::NotFound(format!("trade {trade_id}")))?
        .clone();
    if t.status != TradeStatus::Disputed {
        return Err(VmError::Invalid("trade not in dispute".into()));
    }
    let d = state
        .disputes
        .disputes
        .get(&trade_id)
        .ok_or_else(|| VmError::NotFound("dispute".into()))?;
    if d.ruling.is_some() {
        return Err(VmError::Invalid("dispute already ruled".into()));
    }

    let (mut buyer_share, mut seller_share) = shares(t.amount, ruling);
    // Dispute fee from the loser's share, capped at that share.
    let dispute_fee =
        mul_div_floor(t.amount, state.params.dispute_fee_bps as Amount, 10_000).unwrap_or(0);
    let buyer_loses = buyer_share <= seller_share;
    let charged = if buyer_loses {
        let c = dispute_fee.min(buyer_share);
        buyer_share -= c;
        c
    } else {
        let c = dispute_fee.min(seller_share);
        seller_share -= c;
        c
    };
    let group = format!("dispute:{trade_id}");
    let escrow = escrow_key(t.seller, &t.asset);
    if buyer_share > 0 {
        state.ledger.post(
            &format!("{group}:buyer"),
            TxType::MarketplaceEscrowRelease,
            Some(&group),
            None,
            vec![
                Record::debit(escrow.clone(), buyer_share),
                Record::credit(tokens::deposit_key(t.buyer, &t.asset), buyer_share),
            ],
        )?;
    }
    // The seller's escrowed fee returns with a WinsSeller ruling (no trade
    // happened); otherwise the platform keeps it as on a release.
    let seller_back = match ruling {
        Ruling::WinsSeller => seller_share.saturating_add(t.fee),
        _ => seller_share,
    };
    if seller_back > 0 {
        state.ledger.post(
            &format!("{group}:seller"),
            TxType::MarketplaceEscrowCancel,
            Some(&group),
            None,
            vec![
                Record::debit(escrow.clone(), seller_back),
                Record::credit(tokens::deposit_key(t.seller, &t.asset), seller_back),
            ],
        )?;
    }
    let platform_fee = if matches!(ruling, Ruling::WinsSeller) {
        0
    } else {
        t.fee
    };
    fees::collect(
        state,
        &format!("{group}:fee"),
        TxType::MarketplaceDisputeChargeFee,
        Some(&group),
        escrow.clone(),
        &t.asset,
        platform_fee,
    )?;
    fees::collect(
        state,
        &format!("{group}:dispute_fee"),
        TxType::MarketplaceDisputeChargeFee,
        Some(&group),
        escrow,
        &t.asset,
        charged,
    )?;

    let d = state.disputes.disputes.get_mut(&trade_id).expect("checked");
    d.ruling = Some(ruling);
    d.ruled_by = Some(signer);
    d.ruled_at = Some(ctx.seconds());
    finish_trade(state, trade_id, TradeStatus::Ruled);
    Ok(vec![Event::DisputeRuled {
        trade_id,
        buyer_amount: buyer_share,
        seller_amount: seller_back,
    }])
}

pub fn end_block(_state: &mut State, _ctx: &BlockContext) -> Vec<Event> {
    Vec::new()
}
