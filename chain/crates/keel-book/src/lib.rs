//! The pure matcher. No I/O, no clocks: given the resting book, the synthetic
//! levels a liquidity source offers, and an incoming order, produce the exact
//! fills that would happen. The VM's markets module persists and settles the
//! plan; this crate is unit-tested to the sat.
//!
//! Rules (2026-09-03, unchanged by the chain port):
//! - Price-time priority among user orders (better price first, then `seq`).
//! - Synthetic levels (the house quote) have the LOWEST priority: they fill
//!   only when strictly better than the best user level, or when no user
//!   level crosses. A user order at the same price always fills first.
//! - The taker gets the MAKER's price (price improvement is the taker's).
//! - Self-trade prevention: an incoming order never fills its own owner's
//!   resting orders; they are skipped, not cancelled.
//! - Taker fee only when the maker is a user; house fills earn the spread
//!   embedded in the house price instead. The fee is taken from what the
//!   taker RECEIVES so every ledger leg stays single-asset.
//! - Quantities round DOWN to the lot size; a fill worth zero quote units
//!   is never made.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::collections::{BTreeMap, VecDeque};

use keel_types::{
    base_for_quote, fee_of, floor_to_lot, quote_amount, Address, Amount, FeeSide, OrderId,
    OrderType, PairConfig, Seq, SettleParty, Side,
};
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct RestingOrder {
    pub id: OrderId,
    pub owner: Address,
    pub price: Amount,
    pub remaining: Amount,
    pub seq: Seq,
}

/// One aggregated price level, for snapshots.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Level {
    pub price: Amount,
    pub size: Amount,
    pub orders: usize,
}

/// The resting book of one pair. Bids and asks keyed by price; each level is
/// a queue in `seq` order. On chain this IS the state (there is no database
/// behind it), so it is serializable and mutated only by the VM.
#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Book {
    bids: BTreeMap<Amount, VecDeque<RestingOrder>>,
    asks: BTreeMap<Amount, VecDeque<RestingOrder>>,
}

impl Book {
    pub fn new() -> Self {
        Self::default()
    }

    fn side(&self, side: Side) -> &BTreeMap<Amount, VecDeque<RestingOrder>> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    fn side_mut(&mut self, side: Side) -> &mut BTreeMap<Amount, VecDeque<RestingOrder>> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }

    /// Inserts keeping `seq` order within the level.
    pub fn insert(&mut self, side: Side, order: RestingOrder) {
        if order.remaining == 0 {
            return;
        }
        let level = self.side_mut(side).entry(order.price).or_default();
        let at = level
            .iter()
            .position(|o| o.seq > order.seq)
            .unwrap_or(level.len());
        level.insert(at, order);
    }

    pub fn remove(&mut self, side: Side, price: Amount, id: OrderId) -> Option<RestingOrder> {
        let map = self.side_mut(side);
        let level = map.get_mut(&price)?;
        let at = level.iter().position(|o| o.id == id)?;
        let removed = level.remove(at);
        if level.is_empty() {
            map.remove(&price);
        }
        removed
    }

    /// Sets an order's remaining quantity; zero removes it.
    pub fn set_remaining(&mut self, side: Side, price: Amount, id: OrderId, remaining: Amount) {
        if remaining == 0 {
            self.remove(side, price, id);
            return;
        }
        if let Some(level) = self.side_mut(side).get_mut(&price) {
            if let Some(o) = level.iter_mut().find(|o| o.id == id) {
                o.remaining = remaining;
            }
        }
    }

    /// Resting orders of `side`, best price first, time priority within.
    pub fn best_first(&self, side: Side) -> Vec<RestingOrder> {
        let map = self.side(side);
        let mut out = Vec::new();
        match side {
            Side::Buy => {
                for (_, level) in map.iter().rev() {
                    out.extend(level.iter().cloned());
                }
            }
            Side::Sell => {
                for level in map.values() {
                    out.extend(level.iter().cloned());
                }
            }
        }
        out
    }

    pub fn best_price(&self, side: Side) -> Option<Amount> {
        let map = self.side(side);
        match side {
            Side::Buy => map.keys().next_back().copied(),
            Side::Sell => map.keys().next().copied(),
        }
    }

    /// Aggregated levels, best first, at most `depth`.
    pub fn levels(&self, side: Side, depth: usize) -> Vec<Level> {
        let map = self.side(side);
        let iter: Box<dyn Iterator<Item = (&Amount, &VecDeque<RestingOrder>)>> = match side {
            Side::Buy => Box::new(map.iter().rev()),
            Side::Sell => Box::new(map.iter()),
        };
        iter.take(depth)
            .map(|(price, level)| Level {
                price: *price,
                size: level
                    .iter()
                    .map(|o| o.remaining)
                    .fold(0, Amount::saturating_add),
                orders: level.len(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.bids.is_empty() && self.asks.is_empty()
    }

    pub fn order_count(&self) -> usize {
        self.bids.values().map(VecDeque::len).sum::<usize>()
            + self.asks.values().map(VecDeque::len).sum::<usize>()
    }
}

/// A price level offered by a liquidity source rather than a user order.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct SyntheticLevel {
    pub source: String,
    pub price: Amount,
    pub size: Amount,
    pub party: SettleParty,
}

/// The order being matched. Exactly one of `quantity` / `quote_budget` is
/// set (`quote_budget` only for a market BUY: "spend this much quote").
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Incoming {
    pub owner: Address,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Amount>,
    pub quantity: Option<Amount>,
    pub quote_budget: Option<Amount>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub enum MakerRef {
    User { order_id: OrderId, owner: Address },
    Synthetic { source: String, party: SettleParty },
}

impl MakerRef {
    pub fn is_synthetic(&self) -> bool {
        matches!(self, MakerRef::Synthetic { .. })
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct PlannedFill {
    pub maker: MakerRef,
    pub price: Amount,
    pub quantity: Amount,
    pub quote_amount: Amount,
    pub taker_fee: Amount,
    pub fee_side: FeeSide,
}

#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Plan {
    pub fills: Vec<PlannedFill>,
    pub filled_quantity: Amount,
    pub spent_quote: Amount,
    /// What the taker receives after fees, in the received asset.
    pub received: Amount,
    /// Base units still unfilled (quantity-mode orders).
    pub remaining_quantity: Amount,
    /// Quote units still unspent (budget-mode market buys).
    pub remaining_quote: Amount,
    /// True when a limit order's remainder should rest on the book.
    pub rests: bool,
}

fn crosses(taker: Side, limit: Option<Amount>, level_price: Amount) -> bool {
    match (taker, limit) {
        (_, None) => true,
        (Side::Buy, Some(limit)) => level_price <= limit,
        (Side::Sell, Some(limit)) => level_price >= limit,
    }
}

/// User level wins on ties: for a buying taker a LOWER ask is better, for a
/// selling taker a HIGHER bid is better.
fn user_beats_or_ties(taker: Side, user_price: Amount, house_price: Amount) -> bool {
    match taker {
        Side::Buy => user_price <= house_price,
        Side::Sell => user_price >= house_price,
    }
}

pub fn match_incoming(
    book: &Book,
    synthetic: &[SyntheticLevel],
    incoming: &Incoming,
    cfg: &PairConfig,
) -> Plan {
    let taker = incoming.side;
    let opposite = taker.opposite();
    let bd = cfg.base_decimals;

    // Users' resting orders on the far side, best first, own orders skipped.
    let mut users: Vec<RestingOrder> = book
        .best_first(opposite)
        .into_iter()
        .filter(|o| o.owner != incoming.owner)
        .collect();
    let mut ui = 0usize;

    // Synthetic levels, best first for the taker, with mutable sizes.
    let mut synth: Vec<SyntheticLevel> = synthetic
        .iter()
        .filter(|l| l.size > 0 && l.price > 0)
        .cloned()
        .collect();
    match taker {
        Side::Buy => synth.sort_by_key(|l| l.price),
        Side::Sell => synth.sort_by_key(|l| std::cmp::Reverse(l.price)),
    }
    let mut si = 0usize;

    let budget_mode = incoming.quantity.is_none() && incoming.quote_budget.is_some();
    let mut remaining_qty = incoming.quantity.unwrap_or(0);
    let mut remaining_quote = incoming.quote_budget.unwrap_or(0);

    let mut plan = Plan::default();

    loop {
        let more_wanted = if budget_mode {
            remaining_quote > 0
        } else {
            remaining_qty > 0
        };
        if !more_wanted {
            break;
        }

        let user = users
            .get(ui)
            .filter(|o| crosses(taker, incoming.price, o.price));
        let house = synth
            .get(si)
            .filter(|l| crosses(taker, incoming.price, l.price));

        let pick_user = match (user, house) {
            (Some(u), Some(h)) => user_beats_or_ties(taker, u.price, h.price),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        let (price, level_size) = if pick_user {
            let u = &users[ui];
            (u.price, u.remaining)
        } else {
            let h = &synth[si];
            (h.price, h.size)
        };

        let wanted = if budget_mode {
            base_for_quote(remaining_quote, price, bd)
        } else {
            remaining_qty
        };
        let qty = floor_to_lot(wanted.min(level_size), cfg.lot_size);
        if qty == 0 {
            // Either the taker cannot afford one more lot, or this level holds
            // less than a lot (dust). Try the next level only in the second case.
            if wanted >= cfg.lot_size && level_size < cfg.lot_size {
                if pick_user {
                    ui += 1;
                } else {
                    si += 1;
                }
                continue;
            }
            break;
        }
        let quote = quote_amount(qty, price, bd);
        if quote == 0 {
            break;
        }

        let (maker, fee, fee_side) = if pick_user {
            let u = &users[ui];
            let (fee, fee_side) = match taker {
                Side::Buy => (fee_of(qty, cfg.taker_fee_bps), FeeSide::Base),
                Side::Sell => (fee_of(quote, cfg.taker_fee_bps), FeeSide::Quote),
            };
            (
                MakerRef::User {
                    order_id: u.id,
                    owner: u.owner,
                },
                fee,
                fee_side,
            )
        } else {
            let h = &synth[si];
            let fee_side = match taker {
                Side::Buy => FeeSide::Base,
                Side::Sell => FeeSide::Quote,
            };
            (
                MakerRef::Synthetic {
                    source: h.source.clone(),
                    party: h.party.clone(),
                },
                0,
                fee_side,
            )
        };

        plan.fills.push(PlannedFill {
            maker,
            price,
            quantity: qty,
            quote_amount: quote,
            taker_fee: fee,
            fee_side,
        });
        plan.filled_quantity += qty;
        plan.spent_quote += quote;
        plan.received += match taker {
            Side::Buy => qty - fee,
            Side::Sell => quote - fee,
        };

        if pick_user {
            users[ui].remaining -= qty;
            if users[ui].remaining == 0 {
                ui += 1;
            }
        } else {
            synth[si].size -= qty;
            if synth[si].size == 0 {
                si += 1;
            }
        }
        if budget_mode {
            remaining_quote -= quote;
        } else {
            remaining_qty -= qty;
        }
    }

    plan.remaining_quantity = remaining_qty;
    plan.remaining_quote = remaining_quote;
    plan.rests = incoming.order_type == OrderType::Limit && remaining_qty > 0;
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_types::Asset;

    fn who(tag: u64) -> Address {
        Address::tagged(tag)
    }
    const ALICE: u64 = 1;
    const BOB: u64 = 2;
    const CAROL: u64 = 3;
    const DAVE: u64 = 4;

    fn system_party(t: &str) -> SettleParty {
        SettleParty {
            owner: Address::SYSTEM,
            account_type: t.into(),
        }
    }

    /// BTC-KUSD: 8/6 decimals, tick 0.01, lot 1000 sats, 0.2% taker.
    fn cfg() -> PairConfig {
        PairConfig {
            symbol: "BTC-KUSD".into(),
            base_asset: Asset::new("BTC.BTC"),
            quote_asset: Asset::new("KUSD"),
            base_decimals: 8,
            quote_decimals: 6,
            tick_size: 10_000,
            lot_size: 1_000,
            min_notional: 1_000_000,
            max_notional: 10_000_000_000,
            taker_fee_bps: 20,
            maker_fee_bps: 0,
            enabled: true,
            house_maker_enabled: true,
            house_party: system_party("swap_pool"),
            fee_party: system_party("order_fee"),
        }
    }

    /// Quote price in micro-units per whole BTC; accepts "59999.99".
    fn px(s: &str) -> Amount {
        let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
        let mut frac6 = frac.to_string();
        while frac6.len() < 6 {
            frac6.push('0');
        }
        whole.parse::<Amount>().expect("whole") * 1_000_000 + frac6.parse::<Amount>().expect("frac")
    }

    fn resting(id: u64, user: u64, price: Amount, sats: Amount, seq: u64) -> RestingOrder {
        RestingOrder {
            id: OrderId(id),
            owner: who(user),
            price,
            remaining: sats,
            seq: Seq(seq),
        }
    }

    fn house(price: Amount, sats: Amount) -> SyntheticLevel {
        SyntheticLevel {
            source: "house".into(),
            price,
            size: sats,
            party: system_party("swap_pool"),
        }
    }

    fn limit(user: u64, side: Side, price: Amount, sats: Amount) -> Incoming {
        Incoming {
            owner: who(user),
            side,
            order_type: OrderType::Limit,
            price: Some(price),
            quantity: Some(sats),
            quote_budget: None,
        }
    }

    fn market_qty(user: u64, side: Side, sats: Amount) -> Incoming {
        Incoming {
            owner: who(user),
            side,
            order_type: OrderType::Market,
            price: None,
            quantity: Some(sats),
            quote_budget: None,
        }
    }

    #[test]
    fn price_then_time_priority_among_users() {
        let mut book = Book::new();
        let first = resting(1, ALICE, px("60000"), 1_000_000, 1);
        let second = resting(2, BOB, px("60000"), 1_000_000, 2);
        let cheaper = resting(3, CAROL, px("59990"), 500_000, 3);
        book.insert(Side::Sell, first.clone());
        book.insert(Side::Sell, second.clone());
        book.insert(Side::Sell, cheaper.clone());

        let plan = match_incoming(&book, &[], &market_qty(DAVE, Side::Buy, 2_000_000), &cfg());
        let makers: Vec<OrderId> = plan
            .fills
            .iter()
            .map(|f| match &f.maker {
                MakerRef::User { order_id, .. } => *order_id,
                _ => panic!("user maker expected"),
            })
            .collect();
        assert_eq!(makers, vec![cheaper.id, first.id, second.id]);
        assert_eq!(plan.filled_quantity, 2_000_000);
        assert_eq!(plan.fills[2].quantity, 500_000);
        assert_eq!(plan.remaining_quantity, 0);
    }

    #[test]
    fn house_loses_ties_and_wins_only_when_strictly_better() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(1, ALICE, px("60000"), 1_000_000, 1));
        let tie = [house(px("60000"), 5_000_000)];
        let plan = match_incoming(&book, &tie, &market_qty(BOB, Side::Buy, 1_500_000), &cfg());
        assert!(matches!(plan.fills[0].maker, MakerRef::User { .. }));
        assert!(plan.fills[1].maker.is_synthetic());
        assert_eq!(plan.fills[1].quantity, 500_000);
        let better = [house(px("59999.99"), 5_000_000)];
        let plan = match_incoming(
            &book,
            &better,
            &market_qty(BOB, Side::Buy, 1_500_000),
            &cfg(),
        );
        assert!(plan.fills[0].maker.is_synthetic());
        assert_eq!(plan.fills[0].quantity, 1_500_000);
        assert_eq!(plan.fills.len(), 1);
    }

    #[test]
    fn a_user_order_inside_the_house_spread_fills_first_at_its_own_price() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(1, ALICE, px("60050"), 1_000_000, 1));
        let levels = [house(px("60100"), 9_000_000)];
        let plan = match_incoming(
            &book,
            &levels,
            &limit(BOB, Side::Buy, px("60100"), 2_000_000),
            &cfg(),
        );
        assert_eq!(plan.fills.len(), 2);
        assert_eq!(plan.fills[0].price, px("60050"));
        assert_eq!(plan.fills[1].price, px("60100"));
        assert!(!plan.rests);
        assert_eq!(plan.fills[0].taker_fee, 2_000); // 0.2% of 1 BTC
        assert_eq!(plan.fills[0].fee_side, FeeSide::Base);
        assert_eq!(plan.fills[1].taker_fee, 0);
        assert_eq!(plan.received, 1_998_000);
    }

    #[test]
    fn a_crossing_limit_rests_its_remainder_and_a_market_order_does_not() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(1, ALICE, px("60000"), 1_000_000, 1));
        let plan = match_incoming(
            &book,
            &[],
            &limit(BOB, Side::Buy, px("60000"), 3_000_000),
            &cfg(),
        );
        assert_eq!(plan.filled_quantity, 1_000_000);
        assert_eq!(plan.remaining_quantity, 2_000_000);
        assert!(plan.rests);
        let plan = match_incoming(&book, &[], &market_qty(BOB, Side::Buy, 3_000_000), &cfg());
        assert_eq!(plan.remaining_quantity, 2_000_000);
        assert!(!plan.rests);
        let plan = match_incoming(
            &book,
            &[],
            &limit(BOB, Side::Buy, px("59000"), 1_000_000),
            &cfg(),
        );
        assert!(plan.fills.is_empty());
        assert!(plan.rests);
    }

    #[test]
    fn market_buy_by_quote_budget_spends_at_most_the_budget_in_whole_lots() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(1, ALICE, px("60000"), 100_000_000, 1));
        let incoming = Incoming {
            owner: who(BOB),
            side: Side::Buy,
            order_type: OrderType::Market,
            price: None,
            quantity: None,
            quote_budget: Some(1_000_000_000), // 1,000 KUSD
        };
        let plan = match_incoming(&book, &[], &incoming, &cfg());
        assert_eq!(plan.filled_quantity, 1_666_000);
        assert!(plan.spent_quote <= 1_000_000_000);
        assert_eq!(plan.spent_quote, 999_600_000);
        assert_eq!(plan.remaining_quote, 400_000);
        assert!(!plan.rests);
    }

    #[test]
    fn self_trade_prevention_skips_own_orders_and_rests() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(1, ALICE, px("60000"), 1_000_000, 1));
        book.insert(Side::Sell, resting(2, CAROL, px("60010"), 1_000_000, 2));
        let plan = match_incoming(
            &book,
            &[],
            &limit(ALICE, Side::Buy, px("60010"), 1_500_000),
            &cfg(),
        );
        assert_eq!(plan.fills.len(), 1);
        assert_eq!(plan.fills[0].price, px("60010"));
        assert_eq!(plan.remaining_quantity, 500_000);
        assert!(plan.rests);
        let mut own_only = Book::new();
        own_only.insert(Side::Sell, resting(1, ALICE, px("60000"), 1_000_000, 1));
        let plan = match_incoming(
            &own_only,
            &[],
            &limit(ALICE, Side::Buy, px("60000"), 1_000_000),
            &cfg(),
        );
        assert!(plan.fills.is_empty());
        assert!(plan.rests);
    }

    #[test]
    fn sell_taker_fee_comes_out_of_the_quote_received() {
        let mut book = Book::new();
        book.insert(Side::Buy, resting(1, ALICE, px("60000"), 1_000_000, 1));
        let plan = match_incoming(&book, &[], &market_qty(BOB, Side::Sell, 1_000_000), &cfg());
        assert_eq!(plan.fills[0].quote_amount, 600_000_000);
        assert_eq!(plan.fills[0].taker_fee, 1_200_000);
        assert_eq!(plan.fills[0].fee_side, FeeSide::Quote);
        assert_eq!(plan.received, 598_800_000);
    }

    #[test]
    fn dust_levels_are_skipped_and_sub_lot_remainders_stop() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(1, ALICE, px("60000"), 500, 1));
        book.insert(Side::Sell, resting(2, CAROL, px("60001"), 1_000_000, 2));
        let plan = match_incoming(&book, &[], &market_qty(BOB, Side::Buy, 1_000_500), &cfg());
        assert_eq!(plan.fills.len(), 1);
        assert_eq!(plan.fills[0].price, px("60001"));
        assert_eq!(plan.remaining_quantity, 500);
    }

    #[test]
    fn book_levels_aggregate_best_first() {
        let mut book = Book::new();
        book.insert(Side::Buy, resting(1, ALICE, px("59990"), 1_000, 1));
        book.insert(Side::Buy, resting(2, BOB, px("60000"), 2_000, 2));
        book.insert(Side::Buy, resting(3, CAROL, px("60000"), 3_000, 3));
        let levels = book.levels(Side::Buy, 5);
        assert_eq!(
            levels[0],
            Level {
                price: px("60000"),
                size: 5_000,
                orders: 2
            }
        );
        assert_eq!(levels[1].price, px("59990"));
        assert_eq!(book.best_price(Side::Buy), Some(px("60000")));
        let first = book.best_first(Side::Buy)[0].id;
        book.set_remaining(Side::Buy, px("60000"), first, 0);
        assert_eq!(book.levels(Side::Buy, 5)[0].orders, 1);
        assert_eq!(book.order_count(), 2);
    }

    #[test]
    fn insert_keeps_seq_order_even_when_arriving_out_of_order() {
        let mut book = Book::new();
        book.insert(Side::Sell, resting(2, BOB, px("60000"), 1, 2));
        book.insert(Side::Sell, resting(1, ALICE, px("60000"), 1, 1));
        book.insert(Side::Sell, resting(3, CAROL, px("60000"), 0, 3)); // zero: ignored
        let ids: Vec<u64> = book.best_first(Side::Sell).iter().map(|o| o.id.0).collect();
        assert_eq!(ids, vec![1, 2]);
        assert!(book.remove(Side::Sell, px("60000"), OrderId(1)).is_some());
        assert!(book.remove(Side::Sell, px("60000"), OrderId(1)).is_none());
    }
}
