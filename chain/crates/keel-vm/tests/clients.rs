//! Clients as fee recipients and payers: attestation links an account to its
//! client, the client sets its retail schedule, a taker fill pays the retail
//! leg to the client in the same block, and the chain waives its own
//! protocol fee for the accounts of a client with a running plan and bills
//! the client per flow, in dollars settled in the stable asset.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, ClientFee, PlaceOrder, Proposal, ProposalKind, VoteChoice};
use keel_types::{OrderType, Side};
use keel_vm::modules::clients::Plan;
use keel_vm::{Event, State, VmError};

const KEEL: u128 = 1_000_000;
const USDT: u128 = 1_000_000;

/// Give `client` a running plan with `allowance` USD micro of included
/// usage, without the lock and the purchase that start one.
fn give_plan(state: &mut State, client: keel_types::Address, allowance: u128) {
    state.clients.plans.insert(
        client,
        Plan {
            tier: 1,
            started_secs: T0,
            ends_secs: T0 + 10_000_000,
            fee_usd_micro: 0,
            allowance_usd_micro: allowance,
            allowance_left_usd_micro: allowance,
            ..Default::default()
        },
    );
}

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
        pair: "KEEL-USDT".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(USDT),
        quantity: Some(qty),
        quote_budget: None,
        client_id: None,
    }))]);
    ok(&r[0]);
}

fn market_buy(c: &mut Chain, who: &mut Actor, qty: u128) -> keel_vm::Receipt {
    let (r, _) = c.block(&[who.act(Action::PlaceOrder(PlaceOrder {
        pair: "KEEL-USDT".into(),
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
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);

    // The validator is an attester on the devnet: it vouches for alice,
    // and has a running plan (no allowance: usage comes out of its USDT).
    attest(&mut c, &mut v, &alice, T0 + 1_000_000);
    give_plan(&mut c.state, v.addr(), 0);
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

    // bob rests 10 KEEL at 1 USDT; alice (attested by v) takes 4 KEEL: no
    // protocol fee, the retail leg to v, and v billed the fill usage.
    rest_sell(&mut c, &mut bob, 10 * KEEL);
    let v_keel_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let v_usdt_before = acct(&c.state, v.addr(), &usds(), "deposit");
    let treasury_before = sys(&c.state, &keel(), "treasury");
    let treasury_usdt_before = sys(&c.state, &usds(), "treasury");
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
    // v earned the retail leg in KEEL and paid Keel the fill usage in
    // USDT (a dollar price), which went through the fee split.
    let usage = c.state.clients.params.usage_fill_usd_micro;
    assert!(usage > 0);
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_keel_before + retail as i128
    );
    assert_eq!(
        acct(&c.state, v.addr(), &usds(), "deposit"),
        v_usdt_before - usage as i128
    );
    assert_eq!(
        c.state.clients.earned.get(&(v.addr(), keel())).copied(),
        Some(retail)
    );
    assert!(sys(&c.state, &usds(), "treasury") > treasury_usdt_before);
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
    set_param(&mut c.state, "clients.protocol_fee_waived", 0).expect("param");
    let (r, _) = c.block(&[v.act(Action::SetClientFee(ClientFee {
        p2p_bps: 10,
        taker_bps: 9_999,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    set_param(&mut c.state, "clients.fee_cap_taker_bps", 20).expect("param");
    let (r, _) = c.block(&[v.act(Action::SetClientFee(ClientFee {
        p2p_bps: 100,
        taker_bps: 50,
        withdraw_bps: 10,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(
        set_param(&mut c.state, "clients.fee_cap_taker_bps", 20_000),
        Err(VmError::Invalid(_))
    ));
    rest_sell(&mut c, &mut bob, KEEL);
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), KEEL * 10 / 10_000, "waiver off: fee back");
    audit_clean(&c.state);
}

#[test]
fn fill_usage_is_billed_or_owed_and_rails_close_until_paid() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.clients.params.usage_fill_usd_micro = 2 * USDT;
    state.clients.params.usage_address_usd_micro = USDT;
    state.params.epoch_length_blocks = 50;
    let mut c = Chain::new(state);
    attest(&mut c, &mut v, &alice, T0 + 1_000_000);
    give_plan(&mut c.state, v.addr(), 0);

    // v holds USDT: the fill usage is paid through the fee split.
    rest_sell(&mut c, &mut bob, 10 * KEEL);
    let v_before = acct(&c.state, v.addr(), &usds(), "deposit");
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert!(r.events.contains(&Event::UsageCharged {
        client: v.addr(),
        asset: usds(),
        amount: 2 * USDT,
        kind: "fill".into(),
    }));
    assert_eq!(
        acct(&c.state, v.addr(), &usds(), "deposit"),
        v_before - (2 * USDT) as i128
    );

    // v moves all its USDT away: the next fill still settles for alice,
    // the usage is owed, and v's accounts get no new addresses until paid.
    let v_usdt = acct(&c.state, v.addr(), &usds(), "deposit") as u128;
    let (r, _) = c.block(&[v.act(Action::Transfer(keel_actions::Transfer {
        to: bob.addr(),
        asset: usds(),
        amount: v_usdt,
        memo: None,
    }))]);
    ok(&r[0]);
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), 0);
    assert!(r.events.contains(&Event::UsageOwed {
        client: v.addr(),
        amount: 2 * USDT,
        kind: "fill".into(),
    }));
    assert_eq!(
        c.state.clients.usage_owed.get(&v.addr()).copied(),
        Some(2 * USDT)
    );
    let (r, _) = c.block(&[alice.act(Action::RequestDepositAddress {
        chain: keel_actions::Chain::Tron,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // While v owes, its users are not waived: alice pays the protocol fee
    // and v is billed nothing more.
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), KEEL * 10 / 10_000);
    assert_eq!(
        c.state.clients.usage_owed.get(&v.addr()).copied(),
        Some(2 * USDT)
    );

    // Funded again: the epoch sweep collects what is owed.
    let (r, _) = c.block(&[bob.act(Action::Transfer(keel_actions::Transfer {
        to: v.addr(),
        asset: usds(),
        amount: 10 * USDT,
        memo: None,
    }))]);
    ok(&r[0]);
    let events = c.advance_to(50);
    assert!(events.contains(&Event::UsageCharged {
        client: v.addr(),
        asset: usds(),
        amount: 2 * USDT,
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
fn a_rail_the_client_cannot_pay_in_full_takes_nothing() {
    let (mut state, alice, bob, v) = setup(0);
    state.clients.params.usage_address_usd_micro = 5 * USDT;
    state.clients.attested_by.insert(alice.addr(), v.addr());
    // v owes 4 USDT and holds 6: the owed part alone could be paid, the
    // address price on top of it could not.
    let v_usdt = acct(&state, v.addr(), &usds(), "deposit") as u128;
    let mut c = Chain::new(state);
    let mut v = v;
    let (r, _) = c.block(&[v.act(Action::Transfer(keel_actions::Transfer {
        to: bob.addr(),
        asset: usds(),
        amount: v_usdt - 6 * USDT,
        memo: None,
    }))]);
    ok(&r[0]);
    c.state.clients.usage_owed.insert(v.addr(), 4 * USDT);
    let before = acct(&c.state, v.addr(), &usds(), "deposit");
    assert!(keel_vm::modules::clients::charge_usage(
        &mut c.state,
        alice.addr(),
        "address",
        &[1u8; 32]
    )
    .is_err());
    assert_eq!(acct(&c.state, v.addr(), &usds(), "deposit"), before);
    assert_eq!(
        c.state.clients.usage_owed.get(&v.addr()).copied(),
        Some(4 * USDT),
        "a refused charge must not write the debt down"
    );
    audit_clean(&c.state);
}

/// A client that is not the validator (whose epoch payout would move its
/// balances): carol, registered as an attester and funded by `from`.
fn client(c: &mut Chain, from: &mut Actor, keel_amount: u128, usdt_amount: u128) -> Actor {
    let carol = Actor::new(31);
    c.state.attest.attesters.insert(carol.addr());
    let (r, _) = c.block(&[
        from.act(Action::Transfer(keel_actions::Transfer {
            to: carol.addr(),
            asset: keel(),
            amount: keel_amount,
            memo: None,
        })),
        from.act(Action::Transfer(keel_actions::Transfer {
            to: carol.addr(),
            asset: usds(),
            amount: usdt_amount,
            memo: None,
        })),
    ]);
    ok(&r[0]);
    if usdt_amount > 0 {
        ok(&r[1]);
    }
    carol
}

fn lock(c: &mut Chain, who: &mut Actor, amount: u128) {
    let (r, _) = c.block(&[who.act(Action::LockBudget { amount })]);
    ok(&r[0]);
}

fn plan_started(events: &[Event]) -> Option<(u8, u128)> {
    events.iter().find_map(|e| match e {
        Event::ClientPlanStarted { tier, fee, .. } => Some((*tier, *fee)),
        _ => None,
    })
}

#[test]
fn a_plan_is_bought_starts_at_once_and_its_allowance_pays_first() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    state.clients.params.plan_period_secs = 45;
    state.clients.params.usage_fill_usd_micro = 100 * USDT;
    let tier1 = state.clients.params.tiers[0].clone();
    let mut c = Chain::new(state);
    let mut carol = client(
        &mut c,
        &mut v,
        tier1.lock_keel + KEEL,
        3 * tier1.fee_usd_micro,
    );
    attest(&mut c, &mut carol, &alice, T0 + 1_000_000);
    rest_sell(&mut c, &mut bob, 10_000 * KEEL);

    // Nothing starts by itself: without a purchase alice pays the protocol
    // fee like anyone else, however long carol waits.
    let events = c.advance_to(20);
    assert_eq!(plan_started(&events), None);
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), KEEL * 10 / 10_000);

    // The fee alone does not buy a plan: the tier's KEEL must be locked.
    let before = acct(&c.state, carol.addr(), &usds(), "deposit");
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("locked")));
    assert_eq!(acct(&c.state, carol.addr(), &usds(), "deposit"), before);
    // Nor does anyone but a client buy one.
    let (r, _) = c.block(&[alice.act(Action::BuyPlan { tier: 1 })]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 4 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // Lock and buy in one block: the period starts in that block.
    let (r, _) = c.block(&[
        carol.act(Action::LockBudget {
            amount: tier1.lock_keel,
        }),
        carol.act(Action::BuyPlan { tier: 1 }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    let started = T0 + c.height;
    assert!(
        r[1].events.contains(&Event::ClientPlanStarted {
            client: carol.addr(),
            tier: 1,
            asset: usds(),
            fee: tier1.fee_usd_micro,
            allowance: tier1.allowance_usd_micro,
            ends_at: started + 45,
        }),
        "{:?}",
        r[1].events
    );
    assert_eq!(
        acct(&c.state, carol.addr(), &usds(), "deposit"),
        before - tier1.fee_usd_micro as i128
    );
    // The same tier cannot be bought twice over.
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("runs until")));

    // Waived now, and the fill usage comes out of the allowance: carol's
    // balance does not move.
    let before = acct(&c.state, carol.addr(), &usds(), "deposit");
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), 0);
    assert!(r.events.contains(&Event::UsageCovered {
        client: carol.addr(),
        amount: 100 * USDT,
        kind: "fill".into(),
    }));
    assert_eq!(acct(&c.state, carol.addr(), &usds(), "deposit"), before);

    // Two more fills use the allowance up; the next one comes out of
    // carol's USDT.
    for _ in 0..2 {
        ok(&market_buy(&mut c, &mut alice, KEEL));
    }
    assert_eq!(
        c.state.clients.plans[&carol.addr()].allowance_left_usd_micro,
        0
    );
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert!(r.events.contains(&Event::UsageCharged {
        client: carol.addr(),
        asset: usds(),
        amount: 100 * USDT,
        kind: "fill".into(),
    }));

    // The period ends: the block it ends in renews it, paid again, so
    // there is no gap in which alice pays the protocol fee.
    let before = acct(&c.state, carol.addr(), &usds(), "deposit");
    let last = started - T0 + 44;
    let events = c.advance_to(last);
    assert_eq!(plan_started(&events), None);
    let (_, events) = c.block(&[]);
    assert_eq!(plan_started(&events), Some((1, tier1.fee_usd_micro)));
    assert_eq!(
        acct(&c.state, carol.addr(), &usds(), "deposit"),
        before - tier1.fee_usd_micro as i128
    );
    let r = market_buy(&mut c, &mut alice, KEEL);
    ok(&r);
    assert_eq!(fill_fee(&r), 0);
    audit_clean(&c.state);
}

#[test]
fn a_client_that_cannot_pay_the_fee_has_no_plan_until_it_can() {
    let (mut state, _alice, _bob, mut v) = setup(0);
    state.clients.params.plan_period_secs = 30;
    let tier2 = state.clients.params.tiers[1].clone();
    let mut c = Chain::new(state);
    let mut carol = client(&mut c, &mut v, tier2.lock_keel + KEEL, 0);
    lock(&mut c, &mut carol, tier2.lock_keel);
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 2 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("holds 0")));
    assert!(c.state.clients.plans.is_empty());
    assert!(
        c.state.clients.usage_owed.is_empty(),
        "a plan fee is never owed"
    );

    // Funded for exactly one period: the purchase goes through, and the
    // lock that meets tier 2 can also buy tier 1.
    let pay = |c: &mut Chain, v: &mut Actor, to, amount| {
        let (r, _) = c.block(&[v.act(Action::Transfer(keel_actions::Transfer {
            to,
            asset: usds(),
            amount,
            memo: None,
        }))]);
        ok(&r[0]);
    };
    pay(&mut c, &mut v, carol.addr(), tier2.fee_usd_micro);
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 2 })]);
    ok(&r[0]);
    assert_eq!(plan_started(&r[0].events), Some((2, tier2.fee_usd_micro)));
    assert_eq!(acct(&c.state, carol.addr(), &usds(), "deposit"), 0);

    // At the end of the period carol cannot pay: no plan, nothing owed,
    // and the first block after the money arrives renews it.
    let ends = c.state.clients.plans[&carol.addr()].ends_secs - T0;
    let events = c.advance_to(ends + 5);
    assert_eq!(plan_started(&events), None);
    assert!(
        keel_vm::modules::clients::active_plan(&c.state, &carol.addr(), T0 + c.height).is_none()
    );
    assert!(c.state.clients.usage_owed.is_empty());
    let (r, events) = c.block(&[v.act(Action::Transfer(keel_actions::Transfer {
        to: carol.addr(),
        asset: usds(),
        amount: tier2.fee_usd_micro,
        memo: None,
    }))]);
    ok(&r[0]);
    assert_eq!(plan_started(&events), Some((2, tier2.fee_usd_micro)));
    audit_clean(&c.state);
}

#[test]
fn an_upgrade_takes_what_the_running_period_has_left_off_the_fee() {
    let (mut state, _alice, _bob, mut v) = setup(0);
    state.clients.params.plan_period_secs = 100;
    let (tier1, tier2) = (
        state.clients.params.tiers[0].clone(),
        state.clients.params.tiers[1].clone(),
    );
    let mut c = Chain::new(state);
    let mut carol = client(&mut c, &mut v, tier2.lock_keel + KEEL, 2_000 * USDT);
    lock(&mut c, &mut carol, tier2.lock_keel);
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    ok(&r[0]);
    let bought = c.height;

    // Half the period later, nothing of the allowance used: half the
    // first tier's fee comes off the second's, and a full period starts.
    c.advance_to(bought + 49);
    let before = acct(&c.state, carol.addr(), &usds(), "deposit");
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 2 })]);
    ok(&r[0]);
    let fee = tier2.fee_usd_micro - tier1.fee_usd_micro / 2;
    assert_eq!(plan_started(&r[0].events), Some((2, fee)));
    assert_eq!(
        acct(&c.state, carol.addr(), &usds(), "deposit"),
        before - fee as i128
    );
    let plan = &c.state.clients.plans[&carol.addr()];
    assert_eq!(
        (plan.tier, plan.ends_secs, plan.allowance_left_usd_micro),
        (2, T0 + c.height + 100, tier2.allowance_usd_micro)
    );
    // A downgrade is not a purchase; it is what the plan renews at.
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    audit_clean(&c.state);
}

#[test]
fn a_used_allowance_earns_no_upgrade_credit() {
    let (mut state, _alice, _bob, mut v) = setup(0);
    let (tier1, tier2) = (
        state.clients.params.tiers[0].clone(),
        state.clients.params.tiers[1].clone(),
    );
    state.clients.params.usage_address_usd_micro = tier1.allowance_usd_micro;
    let mut c = Chain::new(state);
    let mut carol = client(&mut c, &mut v, tier2.lock_keel + KEEL, 2_000 * USDT);
    lock(&mut c, &mut carol, tier2.lock_keel);
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    ok(&r[0]);
    keel_vm::modules::clients::charge_usage_to(&mut c.state, carol.addr(), "address", &[7u8; 32])
        .unwrap();
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 2 })]);
    ok(&r[0]);
    assert_eq!(plan_started(&r[0].events), Some((2, tier2.fee_usd_micro)));
    audit_clean(&c.state);
}

#[test]
fn unlocking_below_the_tier_suspends_the_plan_and_renewal_can_be_stopped() {
    let (mut state, alice, _bob, mut v) = setup(0);
    state.clients.params.plan_period_secs = 40;
    let tier1 = state.clients.params.tiers[0].clone();
    let mut c = Chain::new(state);
    let mut carol = client(
        &mut c,
        &mut v,
        tier1.lock_keel + KEEL,
        3 * tier1.fee_usd_micro,
    );
    attest(&mut c, &mut carol, &alice, T0 + 1_000_000);
    let (r, _) = c.block(&[carol.act(Action::SetPlanRenewal { tier: 0 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    lock(&mut c, &mut carol, tier1.lock_keel);
    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    ok(&r[0]);
    let now = |c: &Chain| T0 + c.height;
    assert_eq!(
        keel_vm::modules::clients::waives_fee(&c.state, c.height, &alice.addr(), now(&c)),
        Some(carol.addr())
    );

    // One orca under the tier: the plan no longer counts. Locking it back
    // restores what is left of the period.
    let (r, _) = c.block(&[carol.act(Action::UnlockBudget { amount: 1 })]);
    ok(&r[0]);
    assert_eq!(
        keel_vm::modules::clients::waives_fee(&c.state, c.height, &alice.addr(), now(&c)),
        None
    );
    lock(&mut c, &mut carol, 1);
    assert_eq!(
        keel_vm::modules::clients::waives_fee(&c.state, c.height, &alice.addr(), now(&c)),
        Some(carol.addr())
    );

    // Renewal switched off: the period ends and nothing is charged.
    let (r, _) = c.block(&[carol.act(Action::SetPlanRenewal { tier: 0 })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::ClientPlanRenewalSet {
        client: carol.addr(),
        tier: 0,
    }));
    let before = acct(&c.state, carol.addr(), &usds(), "deposit");
    let ends = c.state.clients.plans[&carol.addr()].ends_secs - T0;
    let events = c.advance_to(ends + 3);
    assert_eq!(plan_started(&events), None);
    assert_eq!(acct(&c.state, carol.addr(), &usds(), "deposit"), before);
    // Switched back on, the next block renews.
    let (r, events) = c.block(&[carol.act(Action::SetPlanRenewal { tier: 1 })]);
    ok(&r[0]);
    assert_eq!(plan_started(&events), Some((1, tier1.fee_usd_micro)));
    audit_clean(&c.state);
}

#[test]
fn a_plan_pays_for_the_actions_of_the_clients_users() {
    let (mut state, _alice, bob, mut v) = setup(0);
    state.clients.params.tiers[0].user_actions_per_day = 3;
    state.clients.params.user_actions_per_user_per_day = 2;
    let tier1 = state.clients.params.tiers[0].clone();
    let mut c = Chain::new(state);
    let mut carol = client(&mut c, &mut v, tier1.lock_keel + KEEL, tier1.fee_usd_micro);
    lock(&mut c, &mut carol, tier1.lock_keel);
    // Two users who hold USDT and not one orca of KEEL.
    let (mut dave, mut erin) = (Actor::new(41), Actor::new(42));
    for who in [&dave, &erin] {
        let (r, _) = c.block(&[v.act(Action::Transfer(keel_actions::Transfer {
            to: who.addr(),
            asset: usds(),
            amount: 10 * USDT,
            memo: None,
        }))]);
        ok(&r[0]);
        attest(&mut c, &mut carol, who, T0 + 1_000_000);
    }
    // A network: no free budget.
    c.state.params.budget.base = 0;
    let send = |who: &mut Actor| {
        who.act(Action::Transfer(keel_actions::Transfer {
            to: bob.addr(),
            asset: usds(),
            amount: 1,
            memo: None,
        }))
    };

    // Attested, but the client has no plan: dave is an address without
    // KEEL like any other.
    let sa = send(&mut dave);
    assert!(matches!(
        keel_vm::check_admission(&c.state, &sa),
        Err(VmError::NotActivated { .. })
    ));
    let (r, _) = c.block(&[sa]);
    assert!(matches!(err(&r[0]), VmError::NotActivated { .. }));

    let (r, _) = c.block(&[carol.act(Action::BuyPlan { tier: 1 })]);
    ok(&r[0]);
    // With the plan, dave acts with no KEEL and no lock, up to his share.
    for _ in 0..2 {
        let sa = send(&mut dave);
        keel_vm::check_admission(&c.state, &sa).expect("the plan pays");
        let (r, _) = c.block(&[sa]);
        ok(&r[0]);
    }
    let (r, _) = c.block(&[send(&mut dave)]);
    assert!(matches!(err(&r[0]), VmError::NotActivated { .. }));
    // The pool is the client's whole allowance for the day: erin gets the
    // one action left in it.
    let (r, _) = c.block(&[send(&mut erin)]);
    ok(&r[0]);
    let (r, _) = c.block(&[send(&mut erin)]);
    assert!(matches!(err(&r[0]), VmError::NotActivated { .. }));
    assert_eq!(c.state.clients.plans[&carol.addr()].user_actions_used, 3);

    // The next day both are back.
    c.height += 86_400;
    let (r, _) = c.block(&[send(&mut dave), send(&mut erin)]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(c.state.clients.plans[&carol.addr()].user_actions_used, 2);
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
    state.clients.params.usage_release_usd_micro = USDT;
    state.clients.params.usage_ruling_usd_micro = 3 * USDT;
    let mut c = Chain::new(state);
    attest(&mut c, &mut v, &alice, T0 + 1_000_000);
    give_plan(&mut c.state, v.addr(), 0);
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
    let v_before = acct(&c.state, v.addr(), &usds(), "deposit");
    let (r, _) = c.block(&[alice.act(Action::ReleaseTrade { trade_id: 0 })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::UsageCharged {
        client: v.addr(),
        asset: usds(),
        amount: USDT,
        kind: "release".into(),
    }));
    assert_eq!(
        acct(&c.state, v.addr(), &usds(), "deposit"),
        v_before - USDT as i128
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
    let v_before = acct(&c.state, v.addr(), &usds(), "deposit");
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
        acct(&c.state, v.addr(), &usds(), "deposit"),
        v_before - (3 * USDT) as i128
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
            key: "clients.usage_fill_usd_micro".into(),
            value: 7 * USDT,
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
    assert_eq!(c.state.clients.params.usage_fill_usd_micro, 7 * USDT);
    // The tier table is three keys per tier.
    set_param(&mut c.state, "clients.tier2_fee_usd_micro", 5 * USDT).expect("tier key");
    assert_eq!(c.state.clients.params.tiers[1].fee_usd_micro, 5 * USDT);
    assert!(set_param(&mut c.state, "clients.tier4_fee_usd_micro", 1).is_err());
    assert!(set_param(&mut c.state, "clients.plan_period_secs", 60).is_err());
    assert!(r[0].events.contains(&Event::ParamChanged {
        key: "clients.usage_fill_usd_micro".into(),
        value: 7 * USDT,
    }));
}

#[test]
fn usage_prices_are_paid_by_the_client_in_the_stable_asset() {
    let (mut state, alice, _bob, v) = setup(0);
    state.clients.params.usage_address_usd_micro = 3 * USDT;
    state.clients.attested_by.insert(alice.addr(), v.addr());
    let before = acct(&state, v.addr(), &usds(), "deposit");
    let treasury_before = sys(&state, &usds(), "treasury");
    let ev =
        keel_vm::modules::clients::charge_usage(&mut state, alice.addr(), "address", &[1u8; 32])
            .unwrap();
    assert_eq!(
        ev,
        vec![Event::UsageCharged {
            client: v.addr(),
            asset: usds(),
            amount: 3 * USDT,
            kind: "address".into(),
        }]
    );
    assert_eq!(
        acct(&state, v.addr(), &usds(), "deposit"),
        before - (3 * USDT) as i128
    );
    assert!(sys(&state, &usds(), "treasury") > treasury_before);
    // No client, no charge; a free price, no charge.
    assert!(keel_vm::modules::clients::charge_usage(
        &mut state,
        _bob.addr(),
        "address",
        &[2u8; 32]
    )
    .unwrap()
    .is_empty());
    state.clients.params.usage_address_usd_micro = 0;
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
