//! Reference prices (2026-11-15): settlement reads a time-weighted average
//! of the fills over `prices.twap_window_secs`, not the last print.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, PlaceOrder};
use keel_types::{OrderType, Side};
use keel_vm::{
    modules::{prices, tokens},
    VmError,
};

const KEEL: u128 = 1_000_000;
const USDT: u128 = 1_000_000;

fn sell(price: u128, qty: u128) -> Action {
    Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-USDT".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(price),
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    })
}

fn buy(qty: u128) -> Action {
    Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-USDT".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    })
}

#[test]
fn settlement_reads_a_time_weighted_price_not_the_last_print() {
    let (mut state, mut alice, mut bob, _v) = setup(0);
    state.gov.param_admin = Some(alice.addr());
    assert_eq!(state.prices.params.twap_window_secs, 600);
    let mut c = Chain::new(state);
    let t0 = 10_000;

    // A fill at 1 USDT: last price and reference agree.
    let (r, _) = c.block_at_secs(t0, &[bob.act(sell(USDT, 10 * KEEL)), alice.act(buy(KEEL))]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(c.state.markets.price_of("KEEL-USDT"), Some(USDT));
    assert_eq!(prices::reference(&c.state, "KEEL-USDT"), Some(USDT));

    // Five minutes later the book is swept and the last fill prints 2 USDT
    // (9 KEEL rest at 1, one more at 2; a 10 KEEL market buy takes both).
    // The book shows 2; the reference has seen 1 for 300 s and 2 for 0 s:
    // still 1.
    let (r, _) = c.block_at_secs(
        t0 + 300,
        &[bob.act(sell(2 * USDT, KEEL)), alice.act(buy(10 * KEEL))],
    );
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(c.state.markets.price_of("KEEL-USDT"), Some(2 * USDT));
    assert_eq!(prices::reference(&c.state, "KEEL-USDT"), Some(USDT));

    // Five more minutes with no fills: 1 for 300 s, 2 for 300 s: 1.5.
    c.block_at_secs(t0 + 600, &[]);
    assert_eq!(c.state.markets.price_of("KEEL-USDT"), Some(2 * USDT));
    assert_eq!(prices::reference(&c.state, "KEEL-USDT"), Some(USDT * 3 / 2));
    // Everything priced in dollars reads the reference.
    assert_eq!(
        tokens::usd_value(&c.state, &keel(), KEEL),
        Some(USDT * 3 / 2)
    );

    // Once the window has passed the old print entirely, the new level is
    // the reference; samples before the window are dropped as fills arrive
    // (the sample current when the window opens is kept).
    c.block_at_secs(t0 + 300 + 600, &[]);
    assert_eq!(prices::reference(&c.state, "KEEL-USDT"), Some(2 * USDT));
    let (r, _) = c.block_at_secs(
        t0 + 300 + 600,
        &[bob.act(sell(2 * USDT, 5 * KEEL)), alice.act(buy(KEEL))],
    );
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(c.state.prices.samples["KEEL-USDT"].len(), 2);

    // The window is a governance parameter; 0 turns the reference off and
    // settlement falls back to the last fill; a week is the ceiling.
    let (r, _) = c.block_at_secs(
        t0 + 1_000,
        &[
            alice.act(Action::SetParam {
                key: "prices.twap_window_secs".into(),
                value: 8 * 86_400,
            }),
            alice.act(Action::SetParam {
                key: "prices.twap_window_secs".into(),
                value: 0,
            }),
        ],
    );
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    ok(&r[1]);
    assert_eq!(prices::reference(&c.state, "KEEL-USDT"), None);
    assert_eq!(tokens::usd_value(&c.state, &keel(), KEEL), Some(2 * USDT));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn a_migrated_chain_keeps_reading_the_last_fill() {
    assert_eq!(prices::PricesState::migrated().params.twap_window_secs, 0);
}
