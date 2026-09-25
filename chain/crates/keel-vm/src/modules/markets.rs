//! Central limit order books. Placement locks funds, matches against the
//! pure book (`keel-book`), settles every fill through the ledger, and
//! rests the remainder, all inside one action so there is no pending
//! state to sweep.
//!
//! Ledger legs (inherited from services/orderbook, ids kept):
//!   order:{id}:lock      deposit -> order_escrow (owner, locked asset)
//!   fill:{id}:base       base from maker escrow (or house party) -> taker deposit
//!   fill:{id}:quote      quote from taker escrow -> maker deposit (or house party)
//!   fill:{id}:fee        taker fee from taker deposit -> fee split
//!   fill:{id}:release    price improvement back to a buy-taker's deposit
//!   order:{id}:unlock    remaining lock back to deposit (cancel / market remainder)

use crate::{
    context::BlockContext,
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::{Action, HouseQuote, PlaceOrder};
use keel_book::{Book, Incoming, MakerRef, Plan, RestingOrder, SyntheticLevel};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{
    base_for_quote, quote_amount, Address, Amount, Asset, FeeSide, OrderId, OrderType, PairConfig,
    Seq, SettleParty, Side,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::{fees, tokens};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct OrderRecord {
    pub id: OrderId,
    pub owner: Address,
    pub pair: String,
    pub side: Side,
    pub order_type: OrderType,
    pub price: Option<Amount>,
    pub quantity: Option<Amount>,
    pub quote_budget: Option<Amount>,
    pub remaining: Amount,
    pub filled: Amount,
    pub status: OrderStatus,
    pub locked_asset: Asset,
    pub locked_amount: Amount,
    pub locked_remaining: Amount,
    pub seq: Seq,
    pub created_height: u64,
    pub client_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct HouseQuoteState {
    pub bid: Option<(Amount, Amount)>,
    pub ask: Option<(Amount, Amount)>,
    pub valid_until: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Market {
    pub cfg: PairConfig,
    pub book: Book,
    pub house: Option<HouseQuoteState>,
    pub last_price: Option<Amount>,
    pub volume_base: Amount,
    pub volume_quote: Amount,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct MarketsState {
    pub pairs: BTreeMap<String, Market>,
    pub orders: BTreeMap<OrderId, OrderRecord>,
    /// Open orders per owner (for caps and RPC).
    pub open_by_owner: BTreeMap<Address, BTreeSet<OrderId>>,
    pub next_order_id: u64,
    pub next_seq: u64,
    pub next_fill_id: u64,
}

impl MarketsState {
    pub fn price_of(&self, symbol: &str) -> Option<Amount> {
        let m = self.pairs.get(symbol)?;
        if let Some(p) = m.last_price {
            return Some(p);
        }
        // Fall back to the house mid, then the book mid.
        if let Some(h) = &m.house {
            if let (Some((b, _)), Some((a, _))) = (h.bid, h.ask) {
                return Some((a + b) / 2);
            }
        }
        match (m.book.best_price(Side::Buy), m.book.best_price(Side::Sell)) {
            (Some(b), Some(a)) => Some((a + b) / 2),
            _ => None,
        }
    }

    pub fn open_orders_of(&self, owner: &Address) -> Vec<&OrderRecord> {
        self.open_by_owner
            .get(owner)
            .map(|ids| ids.iter().filter_map(|id| self.orders.get(id)).collect())
            .unwrap_or_default()
    }
}

/// A pair config with the inherited BTC-USDT defaults: tick 0.01 quote,
/// lot 1000 base units, notional 1..1,000,000,000 quote (the old $10k cap
/// was a swap-desk knob, not a market rule), house = system swap_pool.
pub fn default_pair(
    symbol: &str,
    base: Asset,
    quote: Asset,
    base_decimals: u32,
    quote_decimals: u32,
) -> PairConfig {
    PairConfig {
        symbol: symbol.to_string(),
        base_asset: base,
        quote_asset: quote,
        base_decimals,
        quote_decimals,
        tick_size: 10_000,
        lot_size: 1_000,
        min_notional: 1_000_000,
        max_notional: 1_000_000_000_000_000,
        taker_fee_bps: 10,
        maker_fee_bps: 0,
        enabled: true,
        house_maker_enabled: true,
        house_party: SettleParty {
            owner: Address::SYSTEM,
            account_type: "swap_pool".into(),
        },
        fee_party: SettleParty {
            owner: Address::SYSTEM,
            account_type: "order_fee".into(),
        },
    }
}

/// Governance hook: list or replace a pair's config (book kept if present).
pub fn list_pair(state: &mut State, cfg: PairConfig) {
    let symbol = cfg.symbol.clone();
    match state.markets.pairs.get_mut(&symbol) {
        Some(m) => m.cfg = cfg,
        None => {
            state.markets.pairs.insert(
                symbol,
                Market {
                    cfg,
                    book: Book::new(),
                    house: None,
                    last_price: None,
                    volume_base: 0,
                    volume_quote: 0,
                },
            );
        }
    }
}

pub fn delist_pair(state: &mut State, symbol: &str) {
    if let Some(m) = state.markets.pairs.get_mut(symbol) {
        m.cfg.enabled = false;
    }
}

pub fn apply(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    action: &Action,
) -> Result<Vec<Event>, VmError> {
    match action {
        Action::PlaceOrder(o) => place(state, ctx, signer, tx_id, o),
        Action::CancelOrder { order_id } => cancel(state, ctx, signer, *order_id),
        Action::HouseQuote(q) => quote(state, ctx, signer, q),
        _ => Err(VmError::Invalid("not a markets action".into())),
    }
}

fn party_key(party: &SettleParty, asset: &Asset) -> Result<AccountKey, VmError> {
    AccountKey::new(party.owner, asset.clone(), &party.account_type)
        .ok_or_else(|| VmError::Invalid(format!("unknown account type {}", party.account_type)))
}

fn escrow_key(owner: Address, asset: &Asset) -> AccountKey {
    AccountKey::new(owner, asset.clone(), "order_escrow").expect("catalog type")
}

/// The house's synthetic levels, clamped to what its inventory can pay.
fn synthetic_levels(state: &State, market: &Market, height: u64) -> Vec<SyntheticLevel> {
    let Some(h) = &market.house else {
        return Vec::new();
    };
    if !market.cfg.house_maker_enabled || h.valid_until < height {
        return Vec::new();
    }
    let cfg = &market.cfg;
    let party = &cfg.house_party;
    let mut out = Vec::new();
    if let Some((price, size)) = h.ask {
        // Selling base: bounded by base inventory.
        let inventory = party_key(party, &cfg.base_asset)
            .map(|k| state.ledger.balance(&k).max(0) as Amount)
            .unwrap_or(0);
        let size = size.min(inventory);
        if size > 0 && price > 0 {
            out.push(SyntheticLevel {
                source: "house".into(),
                price,
                size,
                party: party.clone(),
            });
        }
    }
    if let Some((price, size)) = h.bid {
        // Buying base: bounded by quote inventory converted at the bid.
        let inventory = party_key(party, &cfg.quote_asset)
            .map(|k| state.ledger.balance(&k).max(0) as Amount)
            .unwrap_or(0);
        let affordable = base_for_quote(inventory, price, cfg.base_decimals);
        let size = size.min(affordable);
        if size > 0 && price > 0 {
            out.push(SyntheticLevel {
                source: "house_bid".into(),
                price,
                size,
                party: party.clone(),
            });
        }
    }
    out
}

fn validate(cfg: &PairConfig, o: &PlaceOrder) -> Result<(), VmError> {
    if !cfg.enabled {
        return Err(VmError::Invalid("pair disabled".into()));
    }
    match o.order_type {
        OrderType::Limit => {
            let price = o
                .price
                .ok_or_else(|| VmError::Invalid("limit needs price".into()))?;
            let qty = o
                .quantity
                .ok_or_else(|| VmError::Invalid("limit needs quantity".into()))?;
            if price == 0 || (cfg.tick_size > 0 && price % cfg.tick_size != 0) {
                return Err(VmError::Invalid("price not on tick".into()));
            }
            if qty == 0 || (cfg.lot_size > 0 && qty % cfg.lot_size != 0) {
                return Err(VmError::Invalid("quantity not on lot".into()));
            }
            let notional = quote_amount(qty, price, cfg.base_decimals);
            if notional < cfg.min_notional || notional > cfg.max_notional {
                return Err(VmError::Invalid("notional out of bounds".into()));
            }
        }
        OrderType::Market => {
            if o.price.is_some() {
                return Err(VmError::Invalid("market order cannot carry a price".into()));
            }
            match (o.side, o.quantity, o.quote_budget) {
                (_, Some(qty), None) => {
                    if qty == 0 || (cfg.lot_size > 0 && qty % cfg.lot_size != 0) {
                        return Err(VmError::Invalid("quantity not on lot".into()));
                    }
                }
                (Side::Buy, None, Some(budget)) => {
                    if budget < cfg.min_notional || budget > cfg.max_notional {
                        return Err(VmError::Invalid("budget out of bounds".into()));
                    }
                }
                _ => {
                    return Err(VmError::Invalid(
                        "market order needs quantity or (buy) quote_budget".into(),
                    ))
                }
            }
        }
    }
    Ok(())
}

fn place(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    _tx_id: &[u8; 32],
    o: &PlaceOrder,
) -> Result<Vec<Event>, VmError> {
    let market = state
        .markets
        .pairs
        .get(&o.pair)
        .ok_or_else(|| VmError::NotFound(format!("pair {}", o.pair)))?;
    let cfg = market.cfg.clone();
    validate(&cfg, o)?;
    let open = state
        .markets
        .open_by_owner
        .get(&signer)
        .map(|s| s.len())
        .unwrap_or(0);
    if open as u32 >= state.params.max_open_orders_per_pair {
        return Err(VmError::Invalid("too many open orders".into()));
    }

    // What to lock.
    let (locked_asset, locked_amount) = match (o.side, o.order_type) {
        (Side::Buy, OrderType::Limit) => (
            cfg.quote_asset.clone(),
            quote_amount(
                o.quantity.unwrap_or(0),
                o.price.unwrap_or(0),
                cfg.base_decimals,
            ),
        ),
        (Side::Buy, OrderType::Market) => match (o.quantity, o.quote_budget) {
            (None, Some(b)) => (cfg.quote_asset.clone(), b),
            (Some(qty), _) => {
                // Market buy by quantity: lock what the visible liquidity costs.
                let levels = synthetic_levels(state, market, ctx.height);
                let probe = Incoming {
                    owner: signer,
                    side: Side::Buy,
                    order_type: OrderType::Market,
                    price: None,
                    quantity: Some(qty),
                    quote_budget: None,
                };
                let plan = keel_book::match_incoming(&market.book, &levels, &probe, &cfg);
                if plan.spent_quote == 0 {
                    return Err(VmError::Invalid("no liquidity".into()));
                }
                (cfg.quote_asset.clone(), plan.spent_quote)
            }
            _ => return Err(VmError::Invalid("bad market order".into())),
        },
        (Side::Sell, _) => (cfg.base_asset.clone(), o.quantity.unwrap_or(0)),
    };
    if locked_amount == 0 {
        return Err(VmError::Invalid("nothing to lock".into()));
    }

    let id = OrderId(state.markets.next_order_id);
    let seq = Seq(state.markets.next_seq);
    state.ledger.post(
        &format!("order:{id}:lock"),
        TxType::OrderLock,
        Some(&format!("order:{id}")),
        None,
        vec![
            Record::debit(tokens::deposit_key(signer, &locked_asset), locked_amount),
            Record::credit(escrow_key(signer, &locked_asset), locked_amount),
        ],
    )?;
    state.markets.next_order_id += 1;
    state.markets.next_seq += 1;

    let incoming = Incoming {
        owner: signer,
        side: o.side,
        order_type: o.order_type,
        price: o.price,
        quantity: o.quantity,
        quote_budget: if o.quantity.is_some() {
            None
        } else {
            o.quote_budget
        },
    };
    let market = state.markets.pairs.get(&o.pair).expect("checked");
    let levels = synthetic_levels(state, market, ctx.height);
    let plan = keel_book::match_incoming(&market.book, &levels, &incoming, &cfg);

    let mut events = Vec::new();
    let mut lock_used: Amount = 0;
    settle(
        state,
        ctx,
        &cfg,
        id,
        signer,
        &incoming,
        &plan,
        &mut lock_used,
        &mut events,
    )?;

    // Remainder: rest or unlock.
    let lock_left = locked_amount.saturating_sub(lock_used);
    let remaining = if o.quantity.is_some() {
        plan.remaining_quantity
    } else {
        0
    };
    let rests = plan.rests && remaining > 0;
    let mut locked_remaining = lock_left;
    if !rests && lock_left > 0 {
        state.ledger.post(
            &format!("order:{id}:unlock"),
            TxType::OrderUnlock,
            Some(&format!("order:{id}")),
            None,
            vec![
                Record::debit(escrow_key(signer, &locked_asset), lock_left),
                Record::credit(tokens::deposit_key(signer, &locked_asset), lock_left),
            ],
        )?;
        locked_remaining = 0;
    }

    // Ledger done: now mutate module state.
    let market = state.markets.pairs.get_mut(&o.pair).expect("checked");
    let filled = plan.filled_quantity;
    if plan.spent_quote > 0 {
        market.volume_base = market.volume_base.saturating_add(filled);
        market.volume_quote = market.volume_quote.saturating_add(plan.spent_quote);
        if let Some(last) = plan.fills.last() {
            market.last_price = Some(last.price);
        }
    }
    for f in &plan.fills {
        if let MakerRef::User { order_id, .. } = &f.maker {
            let maker = state
                .markets
                .orders
                .get_mut(order_id)
                .expect("resting order has a record");
            maker.remaining = maker.remaining.saturating_sub(f.quantity);
            maker.filled = maker.filled.saturating_add(f.quantity);
            let maker_uses = match maker.side {
                Side::Buy => f.quote_amount,
                Side::Sell => f.quantity,
            };
            maker.locked_remaining = maker.locked_remaining.saturating_sub(maker_uses);
            maker.status = if maker.remaining == 0 {
                OrderStatus::Filled
            } else {
                OrderStatus::PartiallyFilled
            };
            market
                .book
                .set_remaining(maker.side, f.price, *order_id, maker.remaining);
            if maker.remaining == 0 {
                let owner = maker.owner;
                if let Some(set) = state.markets.open_by_owner.get_mut(&owner) {
                    set.remove(order_id);
                }
            }
        }
    }
    let status = if rests {
        if filled > 0 {
            OrderStatus::PartiallyFilled
        } else {
            OrderStatus::Open
        }
    } else if filled > 0 || plan.spent_quote > 0 {
        OrderStatus::Filled
    } else {
        OrderStatus::Cancelled
    };
    if rests {
        let market = state.markets.pairs.get_mut(&o.pair).expect("checked");
        market.book.insert(
            o.side,
            RestingOrder {
                id,
                owner: signer,
                price: o.price.unwrap_or(0),
                remaining,
                seq,
            },
        );
        state
            .markets
            .open_by_owner
            .entry(signer)
            .or_default()
            .insert(id);
    }
    state.markets.orders.insert(
        id,
        OrderRecord {
            id,
            owner: signer,
            pair: o.pair.clone(),
            side: o.side,
            order_type: o.order_type,
            price: o.price,
            quantity: o.quantity,
            quote_budget: o.quote_budget,
            remaining,
            filled,
            status,
            locked_asset,
            locked_amount,
            locked_remaining,
            seq,
            created_height: ctx.height,
            client_id: o.client_id,
        },
    );
    events.insert(
        0,
        Event::OrderAccepted {
            order_id: id.0,
            owner: signer,
            pair: o.pair.clone(),
            resting: remaining,
        },
    );
    if status == OrderStatus::Cancelled {
        events.push(Event::OrderRejected {
            pair: o.pair.clone(),
            reason: "no fill".into(),
        });
    }
    Ok(events)
}

/// Post every fill's legs. `lock_used` accumulates what the taker's lock
/// paid out (quote for buys, base for sells).
#[allow(clippy::too_many_arguments)]
fn settle(
    state: &mut State,
    _ctx: &BlockContext,
    cfg: &PairConfig,
    taker_id: OrderId,
    taker: Address,
    incoming: &Incoming,
    plan: &Plan,
    lock_used: &mut Amount,
    events: &mut Vec<Event>,
) -> Result<(), VmError> {
    let base = &cfg.base_asset;
    let quote = &cfg.quote_asset;
    let stable = state.tokens.stable.clone();
    for f in &plan.fills {
        let fill_id = state.markets.next_fill_id;
        state.markets.next_fill_id += 1;
        let group = format!("fill:{fill_id}");
        let (maker_base_from, maker_quote_to, maker_order_id) = match &f.maker {
            MakerRef::User { order_id, owner } => (
                escrow_key(*owner, base),
                tokens::deposit_key(*owner, quote),
                Some(order_id.0),
            ),
            MakerRef::Synthetic { party, .. } => {
                (party_key(party, base)?, party_key(party, quote)?, None)
            }
        };
        // For a user maker on the BUY side, base comes from the TAKER
        // (seller) and quote from the maker's escrow. Normalise by taker side.
        match incoming.side {
            Side::Buy => {
                // base: maker -> taker; quote: taker escrow -> maker
                state.ledger.post(
                    &format!("{group}:base"),
                    TxType::OrderFill,
                    Some(&group),
                    None,
                    vec![
                        Record::debit(maker_base_from, f.quantity),
                        Record::credit(tokens::deposit_key(taker, base), f.quantity),
                    ],
                )?;
                state.ledger.post(
                    &format!("{group}:quote"),
                    TxType::OrderFill,
                    Some(&group),
                    None,
                    vec![
                        Record::debit(escrow_key(taker, quote), f.quote_amount),
                        Record::credit(maker_quote_to, f.quote_amount),
                    ],
                )?;
                *lock_used = lock_used.saturating_add(f.quote_amount);
                // Price improvement: taker locked at its limit, paid the maker price.
                if let Some(limit) = incoming.price {
                    let locked_for_qty = quote_amount(f.quantity, limit, cfg.base_decimals);
                    let release = locked_for_qty.saturating_sub(f.quote_amount);
                    if release > 0 {
                        state.ledger.post(
                            &format!("{group}:release"),
                            TxType::OrderUnlock,
                            Some(&group),
                            None,
                            vec![
                                Record::debit(escrow_key(taker, quote), release),
                                Record::credit(tokens::deposit_key(taker, quote), release),
                            ],
                        )?;
                        *lock_used = lock_used.saturating_add(release);
                    }
                }
                if f.taker_fee > 0 && f.fee_side == FeeSide::Base {
                    fees::collect(
                        state,
                        &format!("{group}:fee"),
                        TxType::OrderFill,
                        Some(&group),
                        tokens::deposit_key(taker, base),
                        base,
                        f.taker_fee,
                    )?;
                }
            }
            Side::Sell => {
                // base: taker escrow -> maker; quote: maker -> taker
                let (maker_base_to, maker_quote_from) = match &f.maker {
                    MakerRef::User { owner, .. } => {
                        (tokens::deposit_key(*owner, base), escrow_key(*owner, quote))
                    }
                    MakerRef::Synthetic { party, .. } => {
                        (party_key(party, base)?, party_key(party, quote)?)
                    }
                };
                state.ledger.post(
                    &format!("{group}:base"),
                    TxType::OrderFill,
                    Some(&group),
                    None,
                    vec![
                        Record::debit(escrow_key(taker, base), f.quantity),
                        Record::credit(maker_base_to, f.quantity),
                    ],
                )?;
                state.ledger.post(
                    &format!("{group}:quote"),
                    TxType::OrderFill,
                    Some(&group),
                    None,
                    vec![
                        Record::debit(maker_quote_from, f.quote_amount),
                        Record::credit(tokens::deposit_key(taker, quote), f.quote_amount),
                    ],
                )?;
                *lock_used = lock_used.saturating_add(f.quantity);
                if f.taker_fee > 0 && f.fee_side == FeeSide::Quote {
                    fees::collect(
                        state,
                        &format!("{group}:fee"),
                        TxType::OrderFill,
                        Some(&group),
                        tokens::deposit_key(taker, quote),
                        quote,
                        f.taker_fee,
                    )?;
                }
            }
        }
        // Budgets grow with filled USD volume (both sides).
        if *quote == stable {
            let usd_whole = f.quote_amount / keel_types::pow10(cfg.quote_decimals);
            let p = state.params.budget.clone();
            state.account(taker).budget.earn_from_fill(&p, usd_whole);
            if let MakerRef::User { owner, .. } = &f.maker {
                state.account(*owner).budget.earn_from_fill(&p, usd_whole);
            }
        }
        events.push(Event::OrderFilled {
            order_id: taker_id.0,
            maker_order_id,
            pair: cfg.symbol.clone(),
            price: f.price,
            quantity: f.quantity,
            quote: f.quote_amount,
            fee: f.taker_fee,
        });
    }
    Ok(())
}

fn cancel(
    state: &mut State,
    _ctx: &BlockContext,
    signer: Address,
    order_id: OrderId,
) -> Result<Vec<Event>, VmError> {
    let order = state
        .markets
        .orders
        .get(&order_id)
        .cloned()
        .ok_or_else(|| VmError::NotFound(format!("order {order_id}")))?;
    if order.owner != signer {
        return Err(VmError::Unauthorized);
    }
    if !matches!(
        order.status,
        OrderStatus::Open | OrderStatus::PartiallyFilled
    ) {
        return Err(VmError::Invalid("order not open".into()));
    }
    let released = order.locked_remaining;
    if released > 0 {
        state.ledger.post(
            &format!("order:{order_id}:unlock"),
            TxType::OrderUnlock,
            Some(&format!("order:{order_id}")),
            None,
            vec![
                Record::debit(escrow_key(signer, &order.locked_asset), released),
                Record::credit(tokens::deposit_key(signer, &order.locked_asset), released),
            ],
        )?;
    }
    if let Some(m) = state.markets.pairs.get_mut(&order.pair) {
        m.book
            .remove(order.side, order.price.unwrap_or(0), order_id);
    }
    if let Some(set) = state.markets.open_by_owner.get_mut(&signer) {
        set.remove(&order_id);
    }
    let rec = state.markets.orders.get_mut(&order_id).expect("exists");
    rec.status = OrderStatus::Cancelled;
    rec.locked_remaining = 0;
    Ok(vec![Event::OrderCancelled {
        order_id: order_id.0,
        released,
    }])
}

fn quote(
    state: &mut State,
    ctx: &BlockContext,
    signer: Address,
    q: &HouseQuote,
) -> Result<Vec<Event>, VmError> {
    let max_ttl = state.params.house_quote_max_ttl_blocks;
    let operator = state.staking_house_operator();
    let market = state
        .markets
        .pairs
        .get_mut(&q.pair)
        .ok_or_else(|| VmError::NotFound(format!("pair {}", q.pair)))?;
    // The house party owner is the only account allowed to quote. The
    // SYSTEM party is quoted by whoever governance designates as house
    // operator; until then, nobody.
    let allowed = market.cfg.house_party.owner == signer
        || (market.cfg.house_party.owner.is_system() && operator == Some(signer));
    if !allowed {
        return Err(VmError::Unauthorized);
    }
    if q.valid_until <= ctx.height || q.valid_until > ctx.height + max_ttl {
        return Err(VmError::Invalid("valid_until out of range".into()));
    }
    for (p, _) in q.bid.iter().chain(q.ask.iter()) {
        if *p == 0 || (market.cfg.tick_size > 0 && p % market.cfg.tick_size != 0) {
            return Err(VmError::Invalid("quote price not on tick".into()));
        }
    }
    if let (Some((b, _)), Some((a, _))) = (q.bid, q.ask) {
        if b >= a {
            return Err(VmError::Invalid("bid must be below ask".into()));
        }
    }
    market.house = Some(HouseQuoteState {
        bid: q.bid,
        ask: q.ask,
        valid_until: q.valid_until,
    });
    Ok(Vec::new())
}

pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    for m in state.markets.pairs.values_mut() {
        if m.house.as_ref().is_some_and(|h| h.valid_until < ctx.height) {
            m.house = None;
        }
    }
    Vec::new()
}

impl State {
    /// The account governance designated to quote on behalf of the system
    /// house party (`None` until set).
    pub fn staking_house_operator(&self) -> Option<Address> {
        self.gov_house_operator
    }
}
