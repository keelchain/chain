//! The epoch buyback: every payment to Keel ends up as KEEL.
//!
//! Fees are collected in the asset of the flow (KUSD, BTC, USDT, ...) and
//! split into the `treasury`, `validator_rewards` and `burn` system accounts.
//! At each epoch boundary this module sweeps those balances: for each
//! non-KEEL asset with a `KEEL-<asset>` pair it moves the balance to the
//! system account's deposit, places a market buy for KEEL with that budget
//! against the order book, and moves the KEEL bought (and any budget the
//! book could not fill) back. The burn account's KEEL is the burn. What
//! remains in KEEL is what validators are paid and what the treasury holds.
//!
//! Deterministic: same block, same book, same result on every node. The
//! sweep skips an asset when its balance is below the dust floor, when no
//! pair lists it, when the book has no asks, or when the best ask is more
//! than `buyback_max_slippage_bps` above the last price.

use crate::{
    context::BlockContext,
    modules::{markets, tokens},
    receipt::Event,
    state::State,
};
use keel_actions::PlaceOrder;
use keel_crypto::sha256;
use keel_ledger::{Record, TxType};
use keel_types::{mul_div_floor, Address, Amount, Asset, OrderType, Side};

const KINDS: [&str; 3] = ["treasury", "validator_rewards", "burn"];

pub fn end_block(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let len = state.params.epoch_length_blocks.max(1);
    if ctx.height == 0 || !ctx.height.is_multiple_of(len) {
        return Vec::new();
    }
    sweep(state, ctx)
}

/// The pair that buys KEEL with `asset`, if listed.
fn keel_pair(state: &State, keel: &Asset, asset: &Asset) -> Option<String> {
    state
        .markets
        .pairs
        .iter()
        .find(|(_, m)| m.cfg.base_asset == *keel && m.cfg.quote_asset == *asset)
        .map(|(symbol, _)| symbol.clone())
}

pub fn sweep(state: &mut State, ctx: &BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let keel = state.tokens.native.clone();
    let assets: Vec<Asset> = state
        .tokens
        .assets
        .keys()
        .filter(|a| **a != keel)
        .cloned()
        .collect();
    let dust = state.clients.params.buyback_dust_usd_micro as Amount;
    let slippage = state.clients.params.buyback_max_slippage_bps as Amount;
    for kind in KINDS {
        for asset in &assets {
            let from = tokens::system_key(asset, kind);
            let balance = state.ledger.balance(&from).max(0) as Amount;
            if balance == 0 {
                continue;
            }
            match tokens::usd_value(state, asset, balance) {
                Some(usd) if usd >= dust => {}
                _ => continue,
            }
            let Some(pair) = keel_pair(state, &keel, asset) else {
                continue;
            };
            let (best_ask, last) = {
                let m = &state.markets.pairs[&pair];
                (m.book.best_price(Side::Sell), m.last_price)
            };
            let Some(ask) = best_ask else {
                continue;
            };
            if let Some(lp) = last {
                let ceiling = lp.saturating_add(mul_div_floor(lp, slippage, 10_000).unwrap_or(0));
                if ask > ceiling {
                    continue;
                }
            }
            let tag = format!("buyback:{}:{kind}:{}", ctx.height, asset.as_str());
            let sys_asset = tokens::deposit_key(Address::SYSTEM, asset);
            let sys_keel = tokens::deposit_key(Address::SYSTEM, &keel);
            let keel_before = state.ledger.balance(&sys_keel).max(0) as Amount;
            let asset_before = state.ledger.balance(&sys_asset).max(0) as Amount;
            // Budget into the system deposit account, buy, then settle back.
            if state
                .ledger
                .post(
                    &format!("{tag}:fund"),
                    TxType::Sweeping,
                    Some(&tag),
                    None,
                    vec![
                        Record::debit(from.clone(), balance),
                        Record::credit(sys_asset.clone(), balance),
                    ],
                )
                .is_err()
            {
                continue;
            }
            let order = PlaceOrder {
                pair: pair.clone(),
                side: Side::Buy,
                order_type: OrderType::Market,
                price: None,
                quantity: None,
                quote_budget: Some(balance),
                client_id: None,
            };
            let tx_id = sha256(&[tag.as_bytes()]);
            let sp = state.ledger.savepoint();
            let placed = markets::place(state, ctx, Address::SYSTEM, &tx_id, &order);
            if placed.is_err() {
                state.ledger.rollback(sp);
                // Undo the funding move; nothing was bought.
                let _ = state.ledger.post(
                    &format!("{tag}:refund"),
                    TxType::Sweeping,
                    Some(&tag),
                    None,
                    vec![
                        Record::debit(sys_asset.clone(), balance),
                        Record::credit(from.clone(), balance),
                    ],
                );
                continue;
            }
            let keel_after = state.ledger.balance(&sys_keel).max(0) as Amount;
            let asset_after = state.ledger.balance(&sys_asset).max(0) as Amount;
            let bought = keel_after.saturating_sub(keel_before);
            let leftover = asset_after.saturating_sub(asset_before);
            let spent = balance.saturating_sub(leftover);
            // One posting per asset: the ledger keeps every posting in a
            // single asset.
            if bought > 0 {
                let _ = state.ledger.post(
                    &format!("{tag}:settle-keel"),
                    TxType::Sweeping,
                    Some(&tag),
                    None,
                    vec![
                        Record::debit(sys_keel.clone(), bought),
                        Record::credit(tokens::system_key(&keel, kind), bought),
                    ],
                );
            }
            if leftover > 0 {
                let _ = state.ledger.post(
                    &format!("{tag}:settle-rest"),
                    TxType::Sweeping,
                    Some(&tag),
                    None,
                    vec![
                        Record::debit(sys_asset.clone(), leftover),
                        Record::credit(from.clone(), leftover),
                    ],
                );
            }
            state
                .clients
                .last_buyback
                .insert(asset.clone(), (ctx.height, spent, bought));
            events.push(Event::TreasurySwept {
                kind: kind.to_string(),
                asset: asset.clone(),
                spent,
                keel_bought: bought,
            });
        }
    }
    events
}
