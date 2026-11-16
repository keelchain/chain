//! Reference prices: a time-weighted average of each pair's fills over a
//! governed window (`prices.twap_window_secs`), read by everything on the
//! chain that needs a dollar value (`tokens::usd_value`): the small-trade
//! surcharge, the withdrawal flat fee, the budget earned per dollar, the
//! liquidity pool's mark, the buyback. The order book still shows and
//! trades at the last fill; the reference is what settlement reads, so one
//! print cannot set it: moving the reference means holding a price for
//! the whole window against everyone who arbitrages it back. This is the
//! standard practice (Uniswap's TWAP, perpetual exchanges' mark prices,
//! settlement windows on futures exchanges), added here on 2026-11-15.
//!
//! The average is piecewise constant: the price between two fills is the
//! earlier fill's. A pair with no fill inside the window keeps its last
//! price. A window of 0 turns the reference off, and `usd_value` falls back
//! to the last fill; that is also what a snapshot older than schema 11
//! migrates to, so replaying old blocks is unchanged.

use crate::{
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_types::Amount;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Ten minutes.
pub const DEFAULT_TWAP_WINDOW_SECS: u64 = 600;
/// Longest window governance may set: a week.
pub const MAX_TWAP_WINDOW_SECS: u64 = 7 * 86_400;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct PricesParams {
    /// `prices.twap_window_secs`: the averaging window; 0 = off (last fill).
    pub twap_window_secs: u64,
}

impl Default for PricesParams {
    fn default() -> Self {
        Self {
            twap_window_secs: DEFAULT_TWAP_WINDOW_SECS,
        }
    }
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct PricesState {
    pub params: PricesParams,
    /// Per pair, `(block time ms, fill price)` ascending: every fill inside
    /// the window plus the last one before it (the price the window opens
    /// at). One pair's list is bounded by the fills the window can hold.
    pub samples: BTreeMap<String, Vec<(u64, Amount)>>,
}

pub const PARAM_KEYS: &[&str] = &["prices.twap_window_secs"];

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    match key {
        "prices.twap_window_secs" => Some(state.prices.params.twap_window_secs as u128),
        _ => None,
    }
}

pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    match key {
        "prices.twap_window_secs" => {
            state.prices.params.twap_window_secs = u64::try_from(value)
                .ok()
                .filter(|v| *v <= MAX_TWAP_WINDOW_SECS)
                .ok_or_else(|| {
                    VmError::Invalid(format!("{key} must be within 0..={MAX_TWAP_WINDOW_SECS}"))
                })?
        }
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// A fill happened on `symbol` at `price`, at block time `now_ms`.
pub fn record(prices: &mut PricesState, symbol: &str, price: Amount, now_ms: u64) {
    let window_ms = prices.params.twap_window_secs.saturating_mul(1000);
    let list = prices.samples.entry(symbol.to_string()).or_default();
    match list.last_mut() {
        // Several fills in one block: the block's last price stands.
        Some((t, p)) if *t == now_ms => *p = price,
        _ => list.push((now_ms, price)),
    }
    // Keep what the window can see, plus the sample that is current when
    // the window opens.
    let start = now_ms.saturating_sub(window_ms);
    let keep_from = list.iter().rposition(|(t, _)| *t <= start).unwrap_or(0);
    if keep_from > 0 {
        list.drain(..keep_from);
    }
}

/// The time-weighted average price of `symbol` over the window ending at
/// the chain's current block time; `None` when the window is off or the
/// pair never filled.
pub fn reference(state: &State, symbol: &str) -> Option<Amount> {
    let window_ms = state.prices.params.twap_window_secs.saturating_mul(1000);
    if window_ms == 0 {
        return None;
    }
    let samples = state.prices.samples.get(symbol)?;
    let (&(first_t, _), &(_, last_p)) = (samples.first()?, samples.last()?);
    let now = state.timestamp;
    let start = now.saturating_sub(window_ms).max(first_t);
    if now <= start {
        return Some(last_p);
    }
    // Integrate the piecewise-constant price over [start, now].
    let mut weighted: u128 = 0;
    let mut cursor = start;
    let mut current = samples
        .iter()
        .rev()
        .find(|(t, _)| *t <= start)
        .map(|(_, p)| *p)
        .unwrap_or(last_p);
    for (t, p) in samples.iter().filter(|(t, _)| *t > start) {
        let t = (*t).min(now);
        weighted = weighted.saturating_add(current.saturating_mul((t - cursor) as u128));
        cursor = t;
        current = *p;
    }
    weighted = weighted.saturating_add(current.saturating_mul((now - cursor) as u128));
    Some(weighted / (now - start) as u128)
}
