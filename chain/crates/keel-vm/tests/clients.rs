//! Clients as fee recipients: attestation links an account to its client,
//! the client sets a capped retail schedule, and a taker fill pays the
//! retail leg to the client in the same block.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, ClientFee, PlaceOrder};
use keel_types::{OrderType, Side};
use keel_vm::{Event, VmError};

const KEEL: u128 = 1_000_000;
const KUSD: u128 = 1_000_000;

#[test]
fn attester_sets_a_capped_schedule_and_earns_on_taker_fills() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.gov.param_admin = Some(v.addr());
    let mut c = Chain::new(state);

    // The validator is an attester on the devnet: it vouches for alice.
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    assert_eq!(
        c.state.clients.attested_by.get(&alice.addr()),
        Some(&v.addr())
    );

    // Only an attester may set a schedule, and only within the caps.
    let (r, _) = c.block(&[bob.act(Action::SetClientFee(ClientFee {
        p2p_bps: 10,
        taker_bps: 10,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    let (r, _) = c.block(&[v.act(Action::SetClientFee(ClientFee {
        p2p_bps: 10,
        taker_bps: 9_999,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let (r, _) = c.block(&[v.act(Action::SetClientFee(ClientFee {
        p2p_bps: 100,
        taker_bps: 50,
        withdraw_bps: 10,
    }))]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::ClientFeeSet {
        client: v.addr(),
        p2p_bps: 100,
        taker_bps: 50,
        withdraw_bps: 10,
    }));

    // bob rests 10 KEEL at 1 KUSD; alice (attested by v) takes 4 KEEL.
    let (r, _) = c.block(&[bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(KUSD),
        quantity: Some(10 * KEEL),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    let v_keel_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[alice.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(4 * KEEL),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    // Retail: 50 bps of 4 KEEL, in the fee asset of a buy (the base).
    let retail = 4 * KEEL * 50 / 10_000;
    assert!(
        r[0].events.contains(&Event::ClientFeePaid {
            client: v.addr(),
            payer: alice.addr(),
            asset: keel(),
            amount: retail,
            flow: "taker".into(),
        }),
        "{:?}",
        r[0].events
    );
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_keel_before + retail as i128
    );
    assert_eq!(
        c.state.clients.earned.get(&(v.addr(), keel())).copied(),
        Some(retail)
    );
    assert!(c.state.ledger.audit().mismatches.is_empty());

    // bob (no client) pays no retail leg when he takes.
    let (r, _) = c.block(&[alice.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(KUSD),
        quantity: Some(KEEL),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    let (r, _) = c.block(&[bob.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(KEEL),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
    assert!(!r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::ClientFeePaid { .. })));

    // The param admin lowers the taker cap; the old schedule stays in force
    // until the client sets a new one, and a new one above the cap fails.
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "clients.fee_cap_taker_bps".into(),
        value: 20,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[v.act(Action::SetClientFee(ClientFee {
        p2p_bps: 100,
        taker_bps: 50,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "clients.fee_cap_taker_bps".into(),
        value: 20_000,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
}

#[test]
fn usage_prices_are_paid_by_the_client_in_keel() {
    let (mut state, alice, _bob, v) = setup(0);
    state.clients.params.usage_address_keel = 3 * KEEL;
    state.clients.attested_by.insert(alice.addr(), v.addr());
    let before = acct(&state, v.addr(), &keel(), "deposit");
    let treasury_before = sys(&state, &keel(), "treasury");
    let ev =
        keel_vm::modules::clients::charge_usage(&mut state, alice.addr(), "address", &[1u8; 32])
            .unwrap();
    assert_eq!(
        ev,
        vec![Event::UsageCharged {
            client: v.addr(),
            asset: keel(),
            amount: 3 * KEEL,
            kind: "address".into(),
        }]
    );
    assert_eq!(
        acct(&state, v.addr(), &keel(), "deposit"),
        before - (3 * KEEL) as i128
    );
    assert!(sys(&state, &keel(), "treasury") > treasury_before);
    // No client, no charge; a free price, no charge.
    assert!(keel_vm::modules::clients::charge_usage(
        &mut state,
        _bob.addr(),
        "address",
        &[2u8; 32]
    )
    .unwrap()
    .is_empty());
    state.clients.params.usage_address_keel = 0;
    assert!(keel_vm::modules::clients::charge_usage(
        &mut state,
        alice.addr(),
        "address",
        &[3u8; 32]
    )
    .unwrap()
    .is_empty());
    assert!(state.ledger.audit().mismatches.is_empty());
}
