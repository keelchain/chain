//! Per-offer arbitration (docs/how-it-works.md §12). The offer owner names
//! the bonded arbitrator(s) a taker accepts by taking the offer, the way
//! the taker accepts its price, payment window and terms; a dispute on
//! such a trade is ruled by the arbitrator bound to it. Offers that name
//! nobody keep the original rule: any member of the governance-elected set
//! may rule. Naming needs only an arbitrator bond, so an exchange's dispute
//! team can rule its own users' trades the day it bonds; governance can
//! bar an address from ruling anywhere (`BanArbitrator`).
//!
//! Binding: one name on the offer binds at `StartTrade`; several let the
//! taker `ChooseArbitrator` while the trade is still unpaid, and the first
//! named applies at `MarkPaid` if the taker never chose. The list is copied
//! onto the trade when it starts, so a later change to the offer applies
//! only to later trades.

use crate::{
    modules::{p2p::TradeStatus, staking},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, OfferId, TradeId};
use keel_types::Address;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ArbitrationParams {
    /// Most arbitrators one offer may name.
    pub max_named: u32,
}

impl Default for ArbitrationParams {
    fn default() -> Self {
        Self { max_named: 8 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ArbitrationState {
    pub params: ArbitrationParams,
    /// Offer -> the arbitrators its owner named.
    pub offer_arbitrators: BTreeMap<OfferId, Vec<Address>>,
    /// Trade -> the candidates copied from its offer when it started.
    pub trade_candidates: BTreeMap<TradeId, Vec<Address>>,
    /// Trade -> the arbitrator bound to rule its dispute.
    pub trade_arbitrator: BTreeMap<TradeId, Address>,
    /// Addresses governance barred from ruling anywhere.
    pub banned: BTreeSet<Address>,
}

pub const PARAM_KEYS: &[&str] = &["arbitration.max_named"];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    match key {
        "arbitration.max_named" => Some(state.arbitration.params.max_named as u128),
        _ => None,
    }
}

/// `SetParam` for the `arbitration.*` keys.
pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    match key {
        "arbitration.max_named" => {
            state.arbitration.params.max_named = u32::try_from(value)
                .ok()
                .filter(|n| (1..=64).contains(n))
                .ok_or_else(|| VmError::Invalid(format!("{key} must be within 1..=64")))?;
        }
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// Eligible to be named in an offer and to rule: holds at least the
/// minimum arbitrator bond and is not banned. Membership of the fallback
/// set is still governance's decision.
pub fn eligible(state: &State, who: &Address) -> bool {
    !state.arbitration.banned.contains(who) && staking::has_arbitrator_bond(state, who)
}

pub fn apply(state: &mut State, signer: Address, action: &Action) -> Result<Vec<Event>, VmError> {
    match action {
        Action::SetOfferArbitrators {
            offer_id,
            arbitrators,
        } => set_offer(state, signer, *offer_id, arbitrators),
        Action::ChooseArbitrator {
            trade_id,
            arbitrator,
        } => choose(state, signer, *trade_id, *arbitrator),
        _ => Err(VmError::Invalid("not an arbitration action".into())),
    }
}

fn set_offer(
    state: &mut State,
    signer: Address,
    offer_id: OfferId,
    arbitrators: &[Address],
) -> Result<Vec<Event>, VmError> {
    let offer = state
        .p2p
        .offers
        .get(&offer_id)
        .ok_or_else(|| VmError::NotFound(format!("offer {offer_id}")))?;
    if offer.owner != signer {
        return Err(VmError::Unauthorized);
    }
    if offer.closed {
        return Err(VmError::Invalid("offer is closed".into()));
    }
    if arbitrators.len() > state.arbitration.params.max_named as usize {
        return Err(VmError::Invalid(format!(
            "at most {} arbitrators per offer",
            state.arbitration.params.max_named
        )));
    }
    let distinct: BTreeSet<&Address> = arbitrators.iter().collect();
    if distinct.len() != arbitrators.len() {
        return Err(VmError::Invalid("duplicate arbitrator".into()));
    }
    if let Some(a) = arbitrators.iter().find(|a| !eligible(state, a)) {
        return Err(VmError::Invalid(format!(
            "{} holds no arbitrator bond or is banned",
            a.to_hex()
        )));
    }
    if arbitrators.is_empty() {
        state.arbitration.offer_arbitrators.remove(&offer_id);
    } else {
        state
            .arbitration
            .offer_arbitrators
            .insert(offer_id, arbitrators.to_vec());
    }
    Ok(vec![Event::OfferArbitratorsSet {
        offer_id,
        count: arbitrators.len() as u32,
    }])
}

fn choose(
    state: &mut State,
    signer: Address,
    trade_id: TradeId,
    arbitrator: Address,
) -> Result<Vec<Event>, VmError> {
    let t = state
        .p2p
        .trades
        .get(&trade_id)
        .ok_or_else(|| VmError::NotFound(format!("trade {trade_id}")))?;
    if t.status != TradeStatus::Funded {
        return Err(VmError::Invalid(
            "the arbitrator is chosen before the trade is marked paid".into(),
        ));
    }
    let owner = state
        .p2p
        .offers
        .get(&t.offer_id)
        .map(|o| o.owner)
        .ok_or_else(|| VmError::NotFound(format!("offer {}", t.offer_id)))?;
    let taker = if t.buyer == owner { t.seller } else { t.buyer };
    if signer != taker {
        return Err(VmError::Unauthorized);
    }
    if state.arbitration.trade_arbitrator.contains_key(&trade_id) {
        return Err(VmError::Invalid("arbitrator already bound".into()));
    }
    let named = state
        .arbitration
        .trade_candidates
        .get(&trade_id)
        .is_some_and(|c| c.contains(&arbitrator));
    if !named {
        return Err(VmError::Invalid("not named by the offer".into()));
    }
    if !eligible(state, &arbitrator) {
        return Err(VmError::Invalid(
            "arbitrator holds no bond any more or is banned".into(),
        ));
    }
    Ok(bind(state, trade_id, arbitrator))
}

fn bind(state: &mut State, trade_id: TradeId, arbitrator: Address) -> Vec<Event> {
    state
        .arbitration
        .trade_arbitrator
        .insert(trade_id, arbitrator);
    vec![Event::TradeArbitratorBound {
        trade_id,
        arbitrator,
    }]
}

/// `StartTrade` hook: copy the offer's list onto the trade and bind when
/// it names exactly one.
pub(crate) fn on_trade_started(state: &mut State, trade_id: TradeId, offer_id: OfferId) -> Vec<Event> {
    let Some(list) = state.arbitration.offer_arbitrators.get(&offer_id).cloned() else {
        return Vec::new();
    };
    state
        .arbitration
        .trade_candidates
        .insert(trade_id, list.clone());
    match list.as_slice() {
        [one] => bind(state, trade_id, *one),
        _ => Vec::new(),
    }
}

/// `MarkPaid` hook: a trade with candidates and no choice takes the first.
pub(crate) fn on_mark_paid(state: &mut State, trade_id: TradeId) -> Vec<Event> {
    if state.arbitration.trade_arbitrator.contains_key(&trade_id) {
        return Vec::new();
    }
    match state
        .arbitration
        .trade_candidates
        .get(&trade_id)
        .and_then(|c| c.first().copied())
    {
        Some(first) => bind(state, trade_id, first),
        None => Vec::new(),
    }
}

/// Who may rule a dispute on `trade_id`: its bound arbitrator while that
/// one still holds a bond, otherwise any member of the fallback set; a
/// banned address never.
pub fn may_rule(state: &State, trade_id: TradeId, who: &Address) -> bool {
    if state.arbitration.banned.contains(who) {
        return false;
    }
    match state.arbitration.trade_arbitrator.get(&trade_id) {
        Some(a) if eligible(state, a) => a == who,
        _ => staking::is_arbitrator(state, who),
    }
}

/// Governance bars `who` from ruling anywhere.
pub fn ban(state: &mut State, who: Address) -> Vec<Event> {
    state.arbitration.banned.insert(who);
    vec![Event::ArbitratorBanned { member: who }]
}
