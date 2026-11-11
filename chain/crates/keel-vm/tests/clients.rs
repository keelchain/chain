//! Clients as fee recipients and payers: attestation links an account to its
//! client, the client sets its retail schedule, a taker fill pays the retail
//! leg to the client in the same block, and (0.2.0) the chain waives its
//! own protocol fee for attested accounts and bills the client per flow.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, ClientFee, PlaceOrder, Proposal, ProposalKind, VoteChoice};
use keel_types::{OrderType, Side};
use keel_vm::{Event, VmError};

const KEEL: u128 = 1_000_000;
const KUSD: u128 = 1_000_000;

fn attest(c: &mut Chain, v: &mut Actor, who: &Actor, expires_at: u64) {
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: who.addr(),
        tier: 1,
        expires_at,
    })]);
    ok(&r[0]);
}

fn rest_sell(c: &mut Chain, who: &mut Actor, qty: u128) {
    let (r, _) = c.block(&[who.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(KUSD),
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
}

fn market_buy(c: &mut Chain, who: &mut Actor, qty: u128) -> keel_vm::Receipt {
    let (r, _) = c.block(&[who.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-KUSD".into(),
        side: Side::Buy,
        order_type: OrderType::Market,
        price: None,
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    }))]);
    r.into_iter().next().unwrap()
}

fn fill_fee(r: &keel_vm::Receipt) -> u128 {
    r.events
        .iter()
        .find_map(|e| match e {
            Event::OrderFilled { fee, .. } => Some(*fee),
            _ => None,
        })
        .expect("a fill")
}

#[test]
fn attester_sets_a_schedule_and_earns_on_taker_fills() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.gov.param_admin = Some(v.addr());
    let mut c = Chain::new(state);

    // The validator is an attester on the devnet: it vouches for alice.
    attest(&mut c, &mut v, &alice, T0 + 1_000_000);
    assert_eq!(
        c.state.clients.attested_by.get(&alice.addr()),
        Some(&v.addr())
    );

    // Only an attester may set a schedule. With the protocol fee waived
    // (the default since 0.2.0) the schedule is uncapped.
    let (r, _) = c.block(&[bob.act(Action::SetClientFee(ClientFee {
        p2p_bps: 10,
        taker_bps: 10,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
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

    // bob rests 10 KEEL at 1 KUSD; alice (attested by v) takes 4 KEEL: no
    // protocol fee, the retail leg to v, and v billed the fill usage.
    rest_sell(&mut c, &mut bob, 10 * KEEL);
    let v_keel_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let treasury_before = sys(&c.state, &keel(), "treasury");
    let r = market_buy(&mut c, &mut alice, 4 * KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), 0, "attested taker pays no protocol fee");
    let retail = 4 * KEEL * 50 / 10_000;
    assert!(
        r.events.contains(&Event::ClientFeePaid {
            client: v.addr(),
            payer: alice.addr(),
            asset: keel(),
            amount: retail,
            flow: "taker".into(),
        }),
        "{:?}",
        r.events
    );
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_keel_before + retail as i128
    );
    assert_eq!(
        c.state.clients.earned.get(&(v.addr(), keel())).copied(),
        Some(retail)
    );
    assert_eq!(sys(&c.state, &keel(), "treasury"), treasury_before);
    audit_clean(&c.state);

    // bob (no client) pays the protocol fee and no retail leg when he takes.
    rest_sell(&mut c, &mut alice, KEEL);
    let r = market_buy(&mut c, &mut bob, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), KEEL * 10 / 10_000);
    assert!(!r
        .events
        .iter()
        .any(|e| matches!(e, Event::ClientFeePaid { .. })));
    assert!(sys(&c.state, &keel(), "treasury") > treasury_before);

    // Switching the waiver off brings back the capped pre-0.2.0 pricing.
    let (r, _) = c.block(&[v.act(Action::SetParam {
        key: "clients.protocol_fee_waived".into(),
        value: 0,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[v.act(Action::SetClientFee(ClientFee {
        p2p_bps: 10,
        taker_bps: 9_999,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
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
    rest_sell(&mut c, &mut bob, KEEL);
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), KEEL * 10 / 10_000, "waiver off: fee back");
    audit_clean(&c.state);
}

#[test]
fn fill_usage_is_billed_or_owed_and_rails_close_until_paid() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.gov.param_admin = Some(v.addr());
    state.clients.params.usage_fill_keel = 2 * KEEL;
    state.clients.params.usage_address_keel = KEEL;
    state.params.epoch_length_blocks = 50;
    let mut c = Chain::new(state);
    attest(&mut c, &mut v, &alice, T0 + 1_000_000);

    // v holds KEEL: the fill usage is paid through the fee split.
    rest_sell(&mut c, &mut bob, 10 * KEEL);
    let v_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert!(r.events.contains(&Event::UsageCharged {
        client: v.addr(),
        asset: keel(),
        amount: 2 * KEEL,
        kind: "fill".into(),
    }));
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_before - (2 * KEEL) as i128
    );

    // v moves all its KEEL away: the next fill still settles for alice,
    // the usage is owed, and v's accounts get no new addresses until paid.
    let v_keel = acct(&c.state, v.addr(), &keel(), "deposit") as u128;
    let (r, _) = c.block(&[v.act(Action::Transfer(keel_actions::Transfer {
        to: bob.addr(),
        asset: keel(),
        amount: v_keel,
        memo: None,
    }))]);
    ok(&r[0]);
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert!(r.events.contains(&Event::UsageOwed {
        client: v.addr(),
        amount: 2 * KEEL,
        kind: "fill".into(),
    }));
    assert_eq!(
        c.state.clients.usage_owed.get(&v.addr()).copied(),
        Some(2 * KEEL)
    );
    let (r, _) = c.block(&[alice.act(Action::RequestDepositAddress {
        chain: keel_actions::Chain::Tron,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // Funded again: the epoch sweep collects what is owed.
    let (r, _) = c.block(&[bob.act(Action::Transfer(keel_actions::Transfer {
        to: v.addr(),
        asset: keel(),
        amount: 10 * KEEL,
        memo: None,
    }))]);
    ok(&r[0]);
    let events = c.advance_to(50);
    assert!(events.contains(&Event::UsageCharged {
        client: v.addr(),
        asset: keel(),
        amount: 2 * KEEL,
        kind: "owed".into(),
    }));
    assert!(c.state.clients.usage_owed.is_empty());
    let (r, _) = c.block(&[alice.act(Action::RequestDepositAddress {
        chain: keel_actions::Chain::Tron,
    })]);
    ok(&r[0]);
    audit_clean(&c.state);
}

#[test]
fn expired_attestation_waives_nothing() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    // Expires right after the next block (T0 + height seconds).
    attest(&mut c, &mut v, &alice, T0 + 3);
    rest_sell(&mut c, &mut bob, 10 * KEEL);
    c.advance_to(10);
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), KEEL * 10 / 10_000);
    assert!(c.state.clients.usage_owed.is_empty());
    audit_clean(&c.state);
}

#[test]
fn p2p_release_and_ruling_bill_the_client_instead_of_the_fee() {
    use keel_actions::{OfferSpec, Ruling, StartTrade};
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.clients.params.usage_release_keel = KEEL;
    state.clients.params.usage_ruling_keel = 3 * KEEL;
    let mut c = Chain::new(state);
    attest(&mut c, &mut v, &alice, T0 + 1_000_000);
    let spec = OfferSpec {
        side: Side::Sell,
        asset: keel(),
        fiat_currency: "EUR".into(),
        payment_method: "SEPA".into(),
        margin_bps: 100,
        fixed_price: None,
        min_amount: KEEL,
        max_amount: 100 * KEEL,
        payment_window_secs: 1_800,
        country: None,
        min_tier: 0,
        terms: String::new(),
        instructions_hash: [7u8; 32],
    };
    let (r, _) = c.block(&[alice.act(Action::CreateOffer(spec))]);
    ok(&r[0]);
    let start = |amount: u128| {
        Action::StartTrade(StartTrade {
            offer_id: 0,
            amount,
            fiat_amount: 500_000,
            instructions_hash: [9u8; 32],
        })
    };
    // alice (attested) sells: nothing but the amount is escrowed.
    let (r, _) = c.block(&[bob.act(start(10 * KEEL))]);
    ok(&r[0]);
    assert_eq!(c.state.p2p.trades[&0].fee, 0);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "marketplace_escrow"),
        (10 * KEEL) as i128
    );
    let v_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[alice.act(Action::ReleaseTrade { trade_id: 0 })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::UsageCharged {
        client: v.addr(),
        asset: keel(),
        amount: KEEL,
        kind: "release".into(),
    }));
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_before - KEEL as i128
    );
    assert!(c.state.clients.waived_trades.is_empty());

    // A dispute lost by alice: no dispute fee, the ruling is billed to v.
    let (r, _) = c.block(&[bob.act(start(10 * KEEL))]);
    ok(&r[0]);
    let (r, _) = c.block(&[bob.act(Action::MarkPaid {
        trade_id: 1,
        proof_hash: None,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[alice.act(Action::OpenDispute {
        trade_id: 1,
        evidence_hash: [3u8; 32],
    })]);
    ok(&r[0]);
    let bob_before = acct(&c.state, bob.addr(), &keel(), "deposit");
    let v_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[v.act(Action::RuleDispute {
        trade_id: 1,
        ruling: Ruling::WinsBuyer,
    })]);
    ok(&r[0]);
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "deposit"),
        bob_before + (10 * KEEL) as i128,
        "the buyer gets the whole amount, no dispute fee"
    );
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_before - (3 * KEEL) as i128
    );
    audit_clean(&c.state);
}

#[test]
fn dotted_client_params_are_votable() {
    let (mut state, mut alice, _bob, mut v) = setup(100_000_000_000);
    state.params.proposal_deposit = 1_000_000_000;
    state.params.voting_period_blocks = 3;
    state.params.timelock_blocks = 1;
    let mut c = Chain::new(state);
    let id = c.state.gov.next_id;
    let (r, _) = c.block(&[alice.act(Action::Propose(Proposal {
        title: "fill usage".into(),
        description: String::new(),
        kind: ProposalKind::ParamChange {
            key: "clients.usage_fill_keel".into(),
            value: 7 * KEEL,
        },
    }))]);
    ok(&r[0]);
    let p = c.state.gov.proposals[&id].clone();
    let (r, _) = c.block(&[v.act(Action::Vote {
        proposal_id: id,
        choice: VoteChoice::Yes,
    })]);
    ok(&r[0]);
    c.advance_to(p.timelock_end);
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: id })]);
    ok(&r[0]);
    assert_eq!(c.state.clients.params.usage_fill_keel, 7 * KEEL);
    assert!(r[0].events.contains(&Event::ParamChanged {
        key: "clients.usage_fill_keel".into(),
        value: 7 * KEEL,
    }));
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
