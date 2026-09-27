//! The epoch buyback: fee income in KUSD becomes KEEL through the
//! KEEL-KUSD book, the burn share is burned, thin books are skipped, and
//! the whole thing is deterministic.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, PlaceOrder};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, OrderType, Side};
use keel_vm::{modules::tokens, Event, State};

const KEEL: u128 = 1_000_000;
const KUSD: u128 = 1_000_000;

/// Put `amount` KUSD of fee income into a system account, as fees would.
fn fund_system(state: &mut State, kind: &str, amount: u128, tag: &str) {
    state
        .ledger
        .post(
            tag,
            TxType::Genesis,
            None,
            None,
            vec![
                Record::debit(tokens::system_key(&usds(), "issuance"), amount),
                Record::credit(tokens::system_key(&usds(), kind), amount),
            ],
        )
        .unwrap();
}

fn rest_sell(bob: &mut Actor, qty: u128, price: u128) -> keel_actions::SignedAction {
    bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(price),
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    }))
}

#[test]
fn epoch_sweep_buys_keel_and_burns_the_burn_share() {
    let (mut state, _alice, mut bob, _v) = setup(0);
    state.params.epoch_length_blocks = 3;
    fund_system(&mut state, "treasury", 1_000 * KUSD, "fee:t");
    fund_system(&mut state, "burn", 50 * KUSD, "fee:b");
    let mut c = Chain::new(state);
    // bob rests 100 KEEL at 1 KUSD: the book has 100 KUSD of depth.
    let (r, _) = c.block(&[rest_sell(&mut bob, 100 * KEEL, KUSD)]);
    ok(&r[0]);
    let bob_kusd_before = acct(&c.state, bob.addr(), &usds(), "deposit");

    // Height 3 is the epoch boundary: the sweep runs.
    let events = c.advance_to(3);
    let swept: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::TreasurySwept { .. }))
        .collect();
    // The treasury's 1,000 KUSD met 100 KUSD of asks: 100 KEEL bought, the
    // rest returned; the burn's 50 KUSD then found an empty book and waited.
    assert_eq!(swept.len(), 1, "{events:?}");
    assert!(matches!(swept[0], Event::TreasurySwept { kind, .. } if kind == "treasury"));
    let treasury_keel = sys(&c.state, &keel(), "treasury");
    let treasury_kusd = sys(&c.state, &usds(), "treasury");
    assert!(
        treasury_keel > 99 * KEEL as i128,
        "treasury KEEL {treasury_keel}"
    );
    assert_eq!(
        treasury_kusd,
        900 * KUSD as i128,
        "unfilled budget returned"
    );
    assert!(
        acct(&c.state, bob.addr(), &usds(), "deposit") >= bob_kusd_before + 100 * KUSD as i128,
        "bob was paid"
    );
    // The system deposit accounts used for the swap hold nothing afterwards.
    assert_eq!(
        c.state
            .ledger
            .balance(&AccountKey::new(Address::SYSTEM, keel(), "deposit").unwrap()),
        0
    );
    assert_eq!(
        c.state
            .ledger
            .balance(&AccountKey::new(Address::SYSTEM, usds(), "deposit").unwrap()),
        0
    );
    assert!(c.state.ledger.audit().mismatches.is_empty());
    assert!(c.state.clients.last_buyback.contains_key(&usds()));

    // Later in the same epoch nothing runs; at the next boundary bob's new
    // ask is deep enough for the treasury's remaining 900 KUSD and then the
    // burn account's 50 KUSD, and the KEEL the burn bought is burned.
    let (r, _) = c.block(&[rest_sell(&mut bob, 2_000 * KEEL, KUSD)]);
    ok(&r[0]);
    let burn_keel_before = sys(&c.state, &keel(), "burn");
    let events = c.advance_to(6);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::TreasurySwept { kind, .. } if kind == "burn")));
    assert!(sys(&c.state, &keel(), "burn") > burn_keel_before);
    assert_eq!(sys(&c.state, &usds(), "burn"), 0);
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn sweep_skips_empty_books_dust_and_slippage() {
    let (mut state, _alice, mut bob, _v) = setup(0);
    state.params.epoch_length_blocks = 2;
    fund_system(&mut state, "treasury", 1_000 * KUSD, "fee:t");
    let mut c = Chain::new(state);
    // No asks: nothing moves.
    let events = c.advance_to(2);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::TreasurySwept { .. })));
    assert_eq!(sys(&c.state, &usds(), "treasury"), 1_000 * KUSD as i128);

    // An ask far above the last price is skipped once a last price exists.
    let (r, _) = c.block(&[rest_sell(&mut bob, 10 * KEEL, KUSD)]);
    ok(&r[0]);
    let events = c.advance_to(4);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::TreasurySwept { .. })));
    let (r, _) = c.block(&[rest_sell(&mut bob, 10 * KEEL, 3 * KUSD)]);
    ok(&r[0]);
    let kusd_before = sys(&c.state, &usds(), "treasury");
    let events = c.advance_to(6);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::TreasurySwept { .. })),
        "{events:?}"
    );
    assert_eq!(sys(&c.state, &usds(), "treasury"), kusd_before);

    // Dust is left alone.
    let (mut state, _alice, mut bob, _v) = setup(0);
    state.params.epoch_length_blocks = 2;
    fund_system(&mut state, "treasury", KUSD / 10, "fee:dust");
    let mut c = Chain::new(state);
    let (r, _) = c.block(&[rest_sell(&mut bob, 10 * KEEL, KUSD)]);
    ok(&r[0]);
    let events = c.advance_to(2);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::TreasurySwept { .. })));
}

#[test]
fn sweep_is_deterministic() {
    let run = || {
        let (mut state, _alice, mut bob, _v) = setup(0);
        state.params.epoch_length_blocks = 2;
        fund_system(&mut state, "treasury", 500 * KUSD, "fee:t");
        fund_system(&mut state, "validator_rewards", 200 * KUSD, "fee:v");
        let mut c = Chain::new(state);
        let (r, _) = c.block(&[rest_sell(&mut bob, 1_000 * KEEL, KUSD)]);
        ok(&r[0]);
        c.advance_to(4);
        c.state.compute_hash()
    };
    assert_eq!(run(), run());
}
