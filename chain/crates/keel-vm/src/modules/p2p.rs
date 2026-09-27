//! P2P offers and escrow trades (Phase 4, docs/plan.md §3 "keel-market"
//! and §8). The fiat leg is off chain; the chain holds the crypto leg in
//! escrow, exactly like the marketplace's `trade-service.ts`:
//!
//!   start:   seller deposit -> seller marketplace_escrow (amount + fee)
//!   release: escrow -> buyer deposit (amount) + fee split
//!   cancel:  the exact inverse of start
//!
//! Offers lock a refundable KEEL deposit so dead listings cost something to
//! keep; the number of live offers per owner follows the allowance ladder
//! (base, then +step per N completed trades).

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, OfferId, OfferSpec, StartTrade, TradeId};
use keel_crypto::Hash32;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{mul_div_floor, pow10, Address, Amount, Asset, Side};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::{attest, fees, tokens};

pub const MAX_MARGIN_BPS: i32 = 5_000;
pub const MIN_PAYMENT_WINDOW_SECS: u32 = 300;
pub const MAX_TERMS_CHARS: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Offer {
    pub id: OfferId,
    pub owner: Address,
    pub spec: OfferSpec,
    pub created_height: u64,
    pub paused: bool,
    pub closed: bool,
    /// KEEL locked in the owner's `offer_deposit` account for this offer.
    pub deposit: Amount,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum TradeStatus {
    Funded,
    Paid,
    Released,
    Cancelled,
    Disputed,
    Ruled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Trade {
    pub id: TradeId,
    pub offer_id: OfferId,
    pub buyer: Address,
    pub seller: Address,
    pub asset: Asset,
    /// Crypto amount the buyer receives on release.
    pub amount: Amount,
    /// Platform fee, paid by the seller on top of `amount` (escrowed with it).
    pub fee: Amount,
    pub fiat_amount: Amount,
    pub fiat_currency: String,
    pub margin_bps: i32,
    pub started_at: u64,
    /// Seconds since epoch after which the seller may cancel an unpaid trade.
    pub deadline: u64,
    pub paid_at: Option<u64>,
    pub status: TradeStatus,
    pub instructions_hash: Hash32,
}

impl Trade {
    pub fn escrowed(&self) -> Amount {
        self.amount.saturating_add(self.fee)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct P2pState {
    pub offers: BTreeMap<OfferId, Offer>,
    pub trades: BTreeMap<TradeId, Trade>,
    pub next_offer_id: u64,
    pub next_trade_id: u64,
    /// Released or ruled trades per address (both parties).
    pub completed_trades: BTreeMap<Address, u32>,
    /// Live (not closed) offers per owner.
    pub open_offers: BTreeMap<Address, BTreeSet<OfferId>>,
    /// Trades still in Funded/Paid/Disputed, with their payment deadline.
    pub open_trades: BTreeMap<TradeId, u64>,
}

impl P2pState {
    /// Live offers `owner` may hold: base + step per N completed trades.
    pub fn offer_allowance(&self, params: &crate::params::Params, owner: &Address) -> u32 {
        let done = self.completed_trades.get(owner).copied().unwrap_or(0);
        let steps = done
            .checked_div(params.offer_allowance_trades_per_step)
            .unwrap_or(0);
        params
            .offer_allowance_base
            .saturating_add(steps.saturating_mul(params.offer_allowance_step))
    }

    pub fn live_offers_of(&self, owner: &Address) -> usize {
        self.open_offers.get(owner).map(|s| s.len()).unwrap_or(0)
    }

    /// True when an unpaid trade is past its payment window.
    pub fn is_expired(&self, trade_id: TradeId, now_secs: u64) -> bool {
        self.trades
            .get(&trade_id)
            .is_some_and(|t| t.status == TradeStatus::Funded && now_secs > t.deadline)
    }
}

fn escrow_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "marketplace_escrow").expect("catalog type")
}

fn offer_deposit_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "offer_deposit").expect("catalog type")
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::CreateOffer(spec) => create_offer(state, ctx, signer, spec),
        Action::UpdateOffer { offer_id, spec } => update_offer(state, ctx, signer, *offer_id, spec),
        Action::PauseOffer { offer_id, paused } => pause_offer(state, signer, *offer_id, *paused),
        Action::CloseOffer { offer_id } => close_offer(state, signer, *offer_id),
        Action::StartTrade(s) => start_trade(state, ctx, signer, tx_id, s),
        Action::MarkPaid { trade_id, .. } => mark_paid(state, ctx, signer, *trade_id),
        Action::ReleaseTrade { trade_id } => release_trade(state, ctx, signer, *trade_id),
        Action::CancelTrade { trade_id } => cancel_trade(state, ctx, signer, *trade_id),
        _ => Err(VmError::Invalid("not a p2p action".into())),
    }
}

fn validate_spec(state: &State, owner: Address, spec: &OfferSpec) -> Result<(), VmError> {
    if !state.tokens.is_registered(&spec.asset) {
        return Err(VmError::NotFound(format!("asset {}", spec.asset)));
    }
    if spec.min_amount == 0 || spec.max_amount == 0 {
        return Err(VmError::Invalid("amounts must be > 0".into()));
    }
    if spec.min_amount > spec.max_amount {
        return Err(VmError::Invalid("min_amount above max_amount".into()));
    }
    if spec.payment_window_secs < MIN_PAYMENT_WINDOW_SECS {
        return Err(VmError::Invalid("payment window too short".into()));
    }
    if spec.margin_bps.abs() > MAX_MARGIN_BPS {
        return Err(VmError::Invalid("margin out of bounds".into()));
    }
    if spec.terms.chars().count() > MAX_TERMS_CHARS {
        return Err(VmError::Invalid("terms too long".into()));
    }
    if spec.fiat_currency.is_empty() || spec.payment_method.is_empty() {
        return Err(VmError::Invalid(
            "fiat currency and payment method required".into(),
        ));
    }
    if spec.fixed_price == Some(0) {
        return Err(VmError::Invalid("fixed price must be > 0".into()));
    }
    // Fundability (offer-funding.ts): a seller must be able to escrow the
    // largest trade the offer allows. Recorded, not locked.
    if spec.side == Side::Sell && tokens::balance(state, owner, &spec.asset) < spec.max_amount {
        return Err(VmError::NotEnoughFunds);
    }
    Ok(())
}

fn create_offer(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    spec: &OfferSpec,
) -> Result<Vec<Event>, VmError> {
    validate_spec(state, signer, spec)?;
    let allowance = state.p2p.offer_allowance(&state.params, &signer);
    if state.p2p.live_offers_of(&signer) as u32 >= allowance {
        return Err(VmError::Invalid(format!(
            "offer allowance of {allowance} reached"
        )));
    }
    let id = state.p2p.next_offer_id;
    let deposit = state.params.offer_deposit;
    let native = state.tokens.native.clone();
    if deposit > 0 {
        state.ledger.post(
            &format!("offer:{id}:deposit"),
            TxType::OfferDeposit,
            Some(&format!("offer:{id}")),
            None,
            vec![
                Record::debit(tokens::deposit_key(signer, &native), deposit),
                Record::credit(offer_deposit_key(signer, &native), deposit),
            ],
        )?;
    }
    state.p2p.next_offer_id += 1;
    state.p2p.offers.insert(
        id,
        Offer {
            id,
            owner: signer,
            spec: spec.clone(),
            created_height: ctx.height,
            paused: false,
            closed: false,
            deposit,
        },
    );
    state.p2p.open_offers.entry(signer).or_default().insert(id);
    Ok(vec![Event::OfferCreated {
        offer_id: id,
        owner: signer,
    }])
}

fn owned_open_offer(state: &State, signer: Address, offer_id: OfferId) -> Result<&Offer, VmError> {
    let offer = state
        .p2p
        .offers
        .get(&offer_id)
        .ok_or_else(|| VmError::NotFound(format!("offer {offer_id}")))?;
    if offer.owner != signer {
        return Err(VmError::Unauthorized);
    }
    if offer.closed {
        return Err(VmError::Invalid("offer closed".into()));
    }
    Ok(offer)
}

fn update_offer(
    state: &mut State,
    _ctx: &BlockContext,
    signer: Address,
    offer_id: OfferId,
    spec: &OfferSpec,
) -> Result<Vec<Event>, VmError> {
    owned_open_offer(state, signer, offer_id)?;
    validate_spec(state, signer, spec)?;
    let offer = state.p2p.offers.get_mut(&offer_id).expect("checked");
    offer.spec = spec.clone();
    Ok(vec![Event::OfferUpdated { offer_id }])
}

fn pause_offer(
    state: &mut State,
    signer: Address,
    offer_id: OfferId,
    paused: bool,
) -> Result<Vec<Event>, VmError> {
    owned_open_offer(state, signer, offer_id)?;
    let offer = state.p2p.offers.get_mut(&offer_id).expect("checked");
    offer.paused = paused;
    Ok(vec![Event::OfferUpdated { offer_id }])
}

fn close_offer(
    state: &mut State,
    signer: Address,
    offer_id: OfferId,
) -> Result<Vec<Event>, VmError> {
    let deposit = owned_open_offer(state, signer, offer_id)?.deposit;
    let native = state.tokens.native.clone();
    if deposit > 0 {
        state.ledger.post(
            &format!("offer:{offer_id}:refund"),
            TxType::OfferRefund,
            Some(&format!("offer:{offer_id}")),
            None,
            vec![
                Record::debit(offer_deposit_key(signer, &native), deposit),
                Record::credit(tokens::deposit_key(signer, &native), deposit),
            ],
        )?;
    }
    let offer = state.p2p.offers.get_mut(&offer_id).expect("checked");
    offer.closed = true;
    offer.paused = true;
    if let Some(set) = state.p2p.open_offers.get_mut(&signer) {
        set.remove(&offer_id);
    }
    Ok(vec![Event::OfferClosed { offer_id }])
}

/// Seller fee in the traded asset: base bps plus the small-trade surcharge
/// when the USD value is known and below the threshold (fees.ts).
pub fn seller_fee(state: &State, asset: &Asset, amount: Amount) -> Amount {
    let p = &state.params;
    let mut bps = p.p2p_seller_fee_bps as Amount;
    if let Some(usd) = tokens::usd_value(state, asset, amount) {
        if usd < p.p2p_small_trade_usd_micro as Amount {
            bps = bps.saturating_add(p.p2p_small_trade_surcharge_bps as Amount);
        }
    }
    mul_div_floor(amount, bps, 10_000).unwrap_or(0)
}

fn start_trade(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    _tx_id: &[u8; 32],
    s: &StartTrade,
) -> Result<Vec<Event>, VmError> {
    let offer = state
        .p2p
        .offers
        .get(&s.offer_id)
        .ok_or_else(|| VmError::NotFound(format!("offer {}", s.offer_id)))?
        .clone();
    if offer.closed || offer.paused {
        return Err(VmError::Invalid("offer not available".into()));
    }
    if offer.owner == signer {
        return Err(VmError::Invalid("cannot trade with own offer".into()));
    }
    if s.amount < offer.spec.min_amount || s.amount > offer.spec.max_amount {
        return Err(VmError::Invalid("amount outside offer bounds".into()));
    }
    if s.fiat_amount == 0 {
        return Err(VmError::Invalid("fiat_amount must be > 0".into()));
    }
    let now = ctx.seconds();
    if attest::tier_of(state, &signer, now) < offer.spec.min_tier {
        return Err(VmError::Unauthorized);
    }
    if let Some(price) = offer.spec.fixed_price {
        let decimals = state
            .tokens
            .decimals(&offer.spec.asset)
            .ok_or_else(|| VmError::NotFound("asset".into()))?;
        let expected = mul_div_floor(s.amount, price, pow10(decimals))
            .ok_or_else(|| VmError::Invalid("overflow".into()))?;
        if s.fiat_amount.abs_diff(expected) > 1 {
            return Err(VmError::Invalid(format!(
                "fiat_amount must be {expected} at the fixed price"
            )));
        }
    }
    let (buyer, seller) = match offer.spec.side {
        Side::Sell => (signer, offer.owner),
        Side::Buy => (offer.owner, signer),
    };
    let fee = seller_fee(state, &offer.spec.asset, s.amount);
    let escrowed = s
        .amount
        .checked_add(fee)
        .ok_or_else(|| VmError::Invalid("overflow".into()))?;
    let id = state.p2p.next_trade_id;
    state.ledger.post(
        &format!("trade:{id}:prepare"),
        TxType::MarketplaceEscrowPrepare,
        Some(&format!("trade:{id}")),
        None,
        vec![
            Record::debit(tokens::deposit_key(seller, &offer.spec.asset), escrowed),
            Record::credit(escrow_key(seller, &offer.spec.asset), escrowed),
        ],
    )?;
    state.p2p.next_trade_id += 1;
    let deadline = now.saturating_add(offer.spec.payment_window_secs as u64);
    state.p2p.trades.insert(
        id,
        Trade {
            id,
            offer_id: s.offer_id,
            buyer,
            seller,
            asset: offer.spec.asset.clone(),
            amount: s.amount,
            fee,
            fiat_amount: s.fiat_amount,
            fiat_currency: offer.spec.fiat_currency.clone(),
            margin_bps: offer.spec.margin_bps,
            started_at: now,
            deadline,
            paid_at: None,
            status: TradeStatus::Funded,
            instructions_hash: s.instructions_hash,
        },
    );
    state.p2p.open_trades.insert(id, deadline);
    Ok(vec![Event::TradeStarted {
        trade_id: id,
        offer_id: s.offer_id,
        buyer,
        seller,
        amount: s.amount,
    }])
}

fn trade_of(state: &State, trade_id: TradeId) -> Result<&Trade, VmError> {
    state
        .p2p
        .trades
        .get(&trade_id)
        .ok_or_else(|| VmError::NotFound(format!("trade {trade_id}")))
}

fn mark_paid(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    trade_id: TradeId,
) -> Result<Vec<Event>, VmError> {
    let t = trade_of(state, trade_id)?;
    if t.buyer != signer {
        return Err(VmError::Unauthorized);
    }
    if t.status != TradeStatus::Funded {
        return Err(VmError::Invalid("trade not awaiting payment".into()));
    }
    let now = ctx.seconds();
    let t = state.p2p.trades.get_mut(&trade_id).expect("checked");
    t.status = TradeStatus::Paid;
    t.paid_at = Some(now);
    Ok(vec![Event::TradePaid { trade_id }])
}

fn release_trade(
    state: &mut State,
    _ctx: &BlockContext,
    signer: Address,
    trade_id: TradeId,
) -> Result<Vec<Event>, VmError> {
    let t = trade_of(state, trade_id)?.clone();
    if t.seller != signer {
        return Err(VmError::Unauthorized);
    }
    if !matches!(t.status, TradeStatus::Funded | TradeStatus::Paid) {
        return Err(VmError::Invalid("trade cannot be released".into()));
    }
    let group = format!("trade:{trade_id}");
    state.ledger.post(
        &format!("{group}:release"),
        TxType::MarketplaceEscrowRelease,
        Some(&group),
        None,
        vec![
            Record::debit(escrow_key(t.seller, &t.asset), t.amount),
            Record::credit(tokens::deposit_key(t.buyer, &t.asset), t.amount),
        ],
    )?;
    // The seller's client, if any, collects its retail fee from the seller
    // in the same block (referrals are the client's own business).
    let mut retail = super::clients::retail(
        state,
        &format!("{group}:retail"),
        TxType::MarketplaceEscrowRelease,
        Some(&group),
        t.seller,
        &t.asset,
        t.amount,
        super::clients::Flow::P2p,
    )?;
    fees::collect(
        state,
        &format!("{group}:fee"),
        TxType::MarketplaceEscrowRelease,
        Some(&group),
        escrow_key(t.seller, &t.asset),
        &t.asset,
        t.fee,
    )?;
    finish_trade(state, trade_id, TradeStatus::Released);
    retail.push(Event::TradeReleased {
        trade_id,
        to_buyer: t.amount,
        fee: t.fee,
    });
    Ok(retail)
}

fn cancel_trade(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    trade_id: TradeId,
) -> Result<Vec<Event>, VmError> {
    let t = trade_of(state, trade_id)?.clone();
    if t.status != TradeStatus::Funded {
        return Err(VmError::Invalid(
            "only an unpaid trade can be cancelled".into(),
        ));
    }
    let now = ctx.seconds();
    if signer == t.buyer {
        // The buyer may walk away any time before paying.
    } else if now <= t.deadline {
        // The seller (or anyone else) must wait out the payment window.
        return Err(if signer == t.seller {
            VmError::Invalid("payment window still open".into())
        } else {
            VmError::Unauthorized
        });
    }
    // Past the deadline the cancel is permissionless (2026-09-09):
    // an expired trade needs no user key to unwind, so a sweep can sign
    // with any operator key and a non-custodial seller is never stuck.
    let group = format!("trade:{trade_id}");
    state.ledger.reverse(
        &format!("{group}:prepare"),
        &format!("{group}:cancel"),
        TxType::MarketplaceEscrowCancel,
        Some(&group),
    )?;
    let t = state.p2p.trades.get_mut(&trade_id).expect("checked");
    t.status = TradeStatus::Cancelled;
    state.p2p.open_trades.remove(&trade_id);
    Ok(vec![Event::TradeCancelled {
        trade_id,
        by: signer,
    }])
}

/// Close out a trade's bookkeeping after its escrow has been settled.
pub(crate) fn finish_trade(state: &mut State, trade_id: TradeId, status: TradeStatus) {
    if let Some(t) = state.p2p.trades.get_mut(&trade_id) {
        t.status = status;
        let (buyer, seller) = (t.buyer, t.seller);
        *state.p2p.completed_trades.entry(buyer).or_insert(0) += 1;
        *state.p2p.completed_trades.entry(seller).or_insert(0) += 1;
    }
    state.p2p.open_trades.remove(&trade_id);
}

/// Nothing auto-cancels (the marketplace never did either); expiry is a
/// query (`P2pState::is_expired`) that lets the seller cancel.
pub fn end_block(_state: &mut State, _ctx: &BlockContext) -> Vec<Event> {
    Vec::new()
}
