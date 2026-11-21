//! Action capacity by locking KEEL (2026-09-08, the Tron energy model),
//! the activation floor a fresh key must meet before its budget counts
//! (2026-11-15), fee mode (no free budget, pay per action), and the
//! network default since 2026-11-21: no free budget and no fee, capacity
//! from the lock only, with the first lock admitted for free.

mod common;

use common::*;
use keel_actions::{Action, Transfer};
use keel_vm::{
    apply::check_admission,
    modules::{budgets, limits},
    Event, VmError,
};

const KEEL: u128 = 1_000_000;

#[test]
fn locked_keel_grants_capacity_and_comes_back_after_the_delay() {
    let (mut state, mut alice, _bob, _v) = setup(0);
    // No free budget, as on a network: the lock is what carries alice.
    state.params.budget.base = 0;
    state.params.budget.cancel_bonus = 0;
    state.params.budget.per_locked_keel_per_day = 100;
    state.params.budget.unlock_delay_secs = 3 * 86_400;
    let mut c = Chain::new(state);
    let before = acct(&c.state, alice.addr(), &keel(), "deposit");

    // Lock 10 KEEL: capacity 1,000 actions/day, deposit down, lock up.
    let (r, _) = c.block_at_secs(
        1_000,
        &[alice.act(Action::LockBudget { amount: 10 * KEEL })],
    );
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::BudgetLocked {
        owner: alice.addr(),
        amount: 10 * KEEL,
        locked_total: 10 * KEEL
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        before - (10 * KEEL) as i128
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "budget_lock"),
        (10 * KEEL) as i128
    );
    let b = &c
        .state
        .account_ref(&alice.addr())
        .expect("alice exists")
        .budget;
    assert_eq!(
        (b.locked, b.lock_cap(&c.state.params.budget)),
        (10 * KEEL, 1_000)
    );

    // The lock itself was free and its first day of capacity is there at
    // once.
    assert_eq!(b.lock_pool, 1_000);
    let (r, _) = c.block_at_secs(
        1_001,
        &[alice.act(Action::Transfer(keel_actions::Transfer {
            to: _bob.addr(),
            asset: keel(),
            amount: 1,
            memo: None,
        }))],
    );
    ok(&r[0]);
    // With the pool spent it refills over the day: nothing a second
    // later, half of it after 12 h.
    c.state.account(alice.addr()).budget.lock_pool = 0;
    let (r, _) = c.block_at_secs(
        1_002,
        &[alice.act(Action::Transfer(keel_actions::Transfer {
            to: _bob.addr(),
            asset: keel(),
            amount: 1,
            memo: None,
        }))],
    );
    assert!(
        matches!(err(&r[0]), VmError::BudgetExhausted),
        "pool spent, a second refills nothing"
    );
    let half_day = 1_000 + 12 * 3_600;
    let (r, _) = c.block_at_secs(
        half_day,
        &[alice.act(Action::Transfer(keel_actions::Transfer {
            to: _bob.addr(),
            asset: keel(),
            amount: 1,
            memo: None,
        }))],
    );
    ok(&r[0]);
    let b = &c
        .state
        .account_ref(&alice.addr())
        .expect("alice exists")
        .budget;
    assert_eq!(b.lock_pool, 499);

    // Unlock 4 KEEL: queued, capacity shrinks to 600/day now, funds back
    // after the delay and not before.
    let (r, _) = c.block_at_secs(
        half_day + 1,
        &[alice.act(Action::UnlockBudget { amount: 4 * KEEL })],
    );
    ok(&r[0]);
    let ready_at = (half_day + 1 + 3 * 86_400) * 1_000;
    assert!(r[0].events.contains(&Event::BudgetUnlockQueued {
        owner: alice.addr(),
        amount: 4 * KEEL,
        ready_at
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "budget_lock"),
        (6 * KEEL) as i128
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "budget_unlocking"),
        (4 * KEEL) as i128
    );
    let b = &c
        .state
        .account_ref(&alice.addr())
        .expect("alice exists")
        .budget;
    assert_eq!(
        (b.locked, b.lock_cap(&c.state.params.budget)),
        (6 * KEEL, 600)
    );
    let (_, ev) = c.block_at_secs(half_day + 3 * 86_400, &[]);
    assert!(
        !ev.iter().any(|e| matches!(e, Event::BudgetUnlocked { .. })),
        "one second early"
    );
    let (_, ev) = c.block_at_secs(half_day + 1 + 3 * 86_400, &[]);
    assert!(ev.contains(&Event::BudgetUnlocked {
        owner: alice.addr(),
        amount: 4 * KEEL
    }));
    assert_eq!(acct(&c.state, alice.addr(), &keel(), "budget_unlocking"), 0);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        before - (6 * KEEL) as i128 - 2
    );
    audit_clean(&c.state);
}

#[test]
fn lock_refuses_more_than_the_balance_and_zero() {
    let (state, mut alice, _bob, _v) = setup(0);
    let mut c = Chain::new(state);
    let balance = acct(&c.state, alice.addr(), &keel(), "deposit") as u128;
    let (r, _) = c.block(&[
        alice.act(Action::LockBudget { amount: 0 }),
        alice.act(Action::LockBudget {
            amount: balance + 1,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::NotEnoughFunds));
    let (r, _) = c.block(&[alice.act(Action::UnlockBudget { amount: 1 })]);
    assert!(
        matches!(err(&r[0]), VmError::NotEnoughFunds),
        "nothing locked"
    );
}

#[test]
fn buying_budget_costs_keel_and_adds_capacity() {
    let (mut state, mut alice, _bob, _v) = setup(0);
    state.params.budget.price_per_action = 2 * KEEL;
    let mut c = Chain::new(state);
    let before = acct(&c.state, alice.addr(), &keel(), "deposit");
    let earned_before = c
        .state
        .account_ref(&alice.addr())
        .map(|m| m.budget.earned)
        .unwrap_or(0);

    let (r, _) = c.block_at_secs(1_000, &[alice.act(Action::BuyBudget { actions: 10 })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::BudgetPurchased {
        owner: alice.addr(),
        actions: 10,
        paid: 20 * KEEL,
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        before - (20 * KEEL) as i128
    );
    let earned = c
        .state
        .account_ref(&alice.addr())
        .expect("alice exists")
        .budget
        .earned;
    assert_eq!(earned, earned_before + 10);
    assert!(c.state.ledger.audit().mismatches.is_empty());

    // Zero actions is refused; an empty purse cannot pay.
    let (r, _) = c.block_at_secs(1_001, &[alice.act(Action::BuyBudget { actions: 0 })]);
    assert!(
        matches!(r[0].error, Some(VmError::Invalid(_))),
        "{:?}",
        r[0].error
    );
    let (r, _) = c.block_at_secs(
        1_002,
        &[alice.act(Action::BuyBudget {
            actions: 1_000_000_000_000,
        })],
    );
    assert!(r[0].error.is_some());
}

fn transfer(to: keel_types::Address, amount: u128) -> Action {
    Action::Transfer(Transfer {
        to,
        asset: keel(),
        amount,
        memo: None,
    })
}

#[test]
fn a_fresh_key_needs_the_activation_floor_before_its_budget_counts() {
    let (state, mut alice, _bob, _v) = setup(0);
    assert_eq!(
        state.limits.activation_keel,
        limits::DEFAULT_ACTIVATION_KEEL,
        "genesis turns the floor on"
    );
    let mut c = Chain::new(state);
    let mut carol = Actor::new(31);
    let floor = c.state.limits.activation_keel;

    // Nothing in hand: refused at admission and in the block, and the
    // refusal costs the nonce like any other failure.
    let sa = carol.act(Action::BuyBudget { actions: 1 });
    assert!(matches!(
        check_admission(&c.state, &sa),
        Err(VmError::NotActivated { need }) if need == floor
    ));
    let (r, _) = c.block(&[sa]);
    assert!(matches!(err(&r[0]), VmError::NotActivated { .. }));
    assert_eq!(c.state.account_ref(&carol.addr()).expect("carol").nonce, 1);
    assert!(!budgets::is_activated(&c.state, carol.addr()));

    // Just under the floor is still refused; the floor itself is enough.
    let (r, _) = c.block(&[alice.act(transfer(carol.addr(), floor - 1))]);
    ok(&r[0]);
    let (r, _) = c.block(&[carol.act(Action::BuyBudget { actions: 1 })]);
    assert!(matches!(err(&r[0]), VmError::NotActivated { .. }));
    let (r, _) = c.block(&[alice.act(transfer(carol.addr(), 1))]);
    ok(&r[0]);
    assert!(budgets::is_activated(&c.state, carol.addr()));
    let (r, _) = c.block(&[carol.act(Action::BuyBudget { actions: 1 })]);
    ok(&r[0]);

    // Locked KEEL counts towards the floor: lock everything, still active.
    let (r, _) = c.block(&[alice.act(transfer(carol.addr(), 5 * KEEL))]);
    ok(&r[0]);
    let spendable = acct(&c.state, carol.addr(), &keel(), "deposit") as u128;
    let (r, _) = c.block_at_secs(
        2_000,
        &[carol.act(Action::LockBudget { amount: spendable })],
    );
    ok(&r[0]);
    assert_eq!(acct(&c.state, carol.addr(), &keel(), "deposit"), 0);
    assert!(budgets::is_activated(&c.state, carol.addr()));

    // A cancel never needs the floor (it only unwinds), so a key that
    // spent its KEEL can still cancel: here it fails on the missing order,
    // not on activation.
    let mut dave = Actor::new(32);
    let (r, _) = c.block(&[dave.act(Action::CancelOrder {
        order_id: keel_types::OrderId(7),
    })]);
    assert!(matches!(err(&r[0]), VmError::NotFound(_)));

    // The floor is a governance parameter; 0 turns it off.
    set_param(&mut c.state, "budget.activation_keel", 0).expect("param");
    let (r, _) = c.block(&[dave.act(Action::BuyBudget { actions: 1 })]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn fee_mode_charges_keel_per_action_once_the_free_budget_is_zero() {
    let (state, mut alice, mut bob, _v) = setup(0);
    let mut c = Chain::new(state);
    assert_eq!(limits::DEFAULT_ACTION_FEE_KEEL, 0, "no fee by default");
    assert!(!limits::fee_mode(&c.state), "a devnet has a free budget");

    // Governance removes the free budget and prices an action: fee mode.
    let fee = 1_000;
    set_param(&mut c.state, "budget.base", 0).expect("param");
    set_param(&mut c.state, "budget.action_fee_keel", fee).expect("param");
    assert!(limits::fee_mode(&c.state));

    // bob has no earned capacity: every action now costs the fee, routed
    // through the split, and shows as an event ahead of the action's own.
    let before = acct(&c.state, bob.addr(), &keel(), "deposit");
    let treasury_before = acct(&c.state, keel_types::Address::SYSTEM, &keel(), "treasury");
    let sa = bob.act(transfer(alice.addr(), KEEL));
    check_admission(&c.state, &sa).expect("admitted in fee mode");
    let (r, _) = c.block(&[sa]);
    ok(&r[0]);
    assert_eq!(
        r[0].events[0],
        Event::ActionFeePaid {
            owner: bob.addr(),
            amount: fee
        }
    );
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "deposit"),
        before - KEEL as i128 - fee as i128
    );
    assert!(acct(&c.state, keel_types::Address::SYSTEM, &keel(), "treasury") > treasury_before);

    // A refused action still paid: the fee is taken before the module runs.
    let before = acct(&c.state, bob.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[bob.act(Action::CancelOrder {
        order_id: keel_types::OrderId(99),
    })]);
    assert!(matches!(err(&r[0]), VmError::NotFound(_)));
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "deposit"),
        before - fee as i128
    );

    // Earned capacity is spent first; the fee only when it is gone.
    c.state.account(bob.addr()).budget.purchase(1);
    let before = acct(&c.state, bob.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[bob.act(transfer(alice.addr(), 1))]);
    ok(&r[0]);
    assert!(!r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::ActionFeePaid { .. })));
    assert_eq!(acct(&c.state, bob.addr(), &keel(), "deposit"), before - 1);

    // The fee is a governance parameter; a key that cannot pay it is
    // refused as exhausted, at admission and in the block.
    set_param(&mut c.state, "budget.action_fee_keel", 10 * KEEL).expect("param");
    let mut poor = Actor::new(41);
    let (r, _) = c.block(&[alice.act(transfer(poor.addr(), c.state.limits.activation_keel))]);
    ok(&r[0]);
    let sa = poor.act(transfer(alice.addr(), 1));
    assert!(matches!(
        check_admission(&c.state, &sa),
        Err(VmError::BudgetExhausted)
    ));
    let (r, _) = c.block(&[sa]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    // A fee of zero in fee mode means: earned or locked capacity only.
    set_param(&mut c.state, "budget.action_fee_keel", 0).expect("param");
    let (r, _) = c.block(&[poor.act(transfer(alice.addr(), 1))]);
    assert!(matches!(err(&r[0]), VmError::BudgetExhausted));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn every_former_constant_is_a_governance_parameter() {
    let (state, mut alice, _bob, _v) = setup(0);
    let mut c = Chain::new(state);
    for key in limits::PARAM_KEYS {
        let before = limits::get_param(&c.state, key).expect(key);
        let value = before + 1;
        set_param(&mut c.state, key, value).expect("param");
        assert_eq!(limits::get_param(&c.state, key), Some(value), "{key}");
    }
    // Out-of-range values are refused, so a cap of zero cannot lock the
    // chain out of offers or proposals.
    assert!(matches!(
        set_param(&mut c.state, "gov.max_title_chars", 0),
        Err(VmError::Invalid(_))
    ));
    // The proposal text limit is applied from the parameter.
    set_param(&mut c.state, "gov.max_title_chars", 3).expect("param");
    let (r, _) = c.block(&[alice.act(Action::Propose(keel_actions::Proposal {
        title: "four".into(),
        description: String::new(),
        kind: keel_actions::ProposalKind::Text,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("length")));
}

/// A network's parameters on the test genesis: no free budget, no fee.
fn network(state: &mut keel_vm::State) {
    state.params = keel_vm::Params::default();
    assert_eq!(state.params.budget.base, 0);
    assert_eq!(state.limits.action_fee_keel, 0);
}

#[test]
fn without_a_free_budget_the_first_lock_is_the_way_in() {
    let (mut state, mut alice, bob, _v) = setup(0);
    network(&mut state);
    let mut c = Chain::new(state);
    let min = c.state.limits.free_lock_min_keel;
    assert_eq!(min, KEEL);

    // Funded and activated, alice still cannot act: there is nothing free.
    let sa = alice.act(transfer(bob.addr(), 1));
    assert!(matches!(
        check_admission(&c.state, &sa),
        Err(VmError::BudgetExhausted)
    ));
    let (r, _) = c.block(&[sa]);
    assert!(matches!(err(&r[0]), VmError::BudgetExhausted));

    // A lock below the minimum is an action like any other.
    let (r, _) = c.block(&[alice.act(Action::LockBudget { amount: min - 1 })]);
    assert!(matches!(err(&r[0]), VmError::BudgetExhausted));

    // The minimum is admitted with no capacity at all, and 1 KEEL is a
    // day's hundred actions, there at once.
    let sa = alice.act(Action::LockBudget { amount: min });
    check_admission(&c.state, &sa).expect("the first lock is admitted");
    let (r, _) = c.block(&[sa]);
    ok(&r[0]);
    let b = &c.state.account_ref(&alice.addr()).expect("alice").budget;
    assert_eq!((b.locked, b.lock_pool, b.used), (KEEL, 100, 0));
    let (r, _) = c.block(&[alice.act(transfer(bob.addr(), 1))]);
    ok(&r[0]);
    assert_eq!(
        c.state
            .account_ref(&alice.addr())
            .expect("alice")
            .budget
            .lock_pool,
        99
    );

    // The free lock still counts against the per-block cap.
    c.state.params.budget.max_per_block = 2;
    let (r, _) = c.block(&[
        alice.act(Action::LockBudget { amount: min }),
        alice.act(Action::LockBudget { amount: min }),
        alice.act(Action::LockBudget { amount: min }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert!(matches!(err(&r[2]), VmError::BlockCap));

    // 0 turns the free lock off.
    set_param(&mut c.state, "budget.free_lock_min_keel", 0).expect("param");
    let mut carol = Actor::new(31);
    c.state.account(alice.addr()).budget.lock_pool = 10;
    let (r, _) = c.block(&[alice.act(transfer(carol.addr(), 5 * KEEL))]);
    ok(&r[0]);
    let (r, _) = c.block(&[carol.act(Action::LockBudget { amount: KEEL })]);
    assert!(matches!(err(&r[0]), VmError::BudgetExhausted));
    audit_clean(&c.state);
}

#[test]
fn a_lock_the_signer_cannot_pay_is_not_free() {
    let (mut state, mut alice, _bob, _v) = setup(0);
    network(&mut state);
    let mut c = Chain::new(state);
    // carol holds the activation floor and half a KEEL: a 1 KEEL lock
    // would be refused by the ledger, so it is not admitted for free and
    // a flood of such locks costs capacity carol does not have.
    let mut carol = Actor::new(31);
    let (r, _) = c.block(&[alice.act(Action::LockBudget { amount: KEEL })]);
    ok(&r[0]);
    let (r, _) = c.block(&[alice.act(transfer(carol.addr(), KEEL / 2))]);
    ok(&r[0]);
    let sa = carol.act(Action::LockBudget { amount: KEEL });
    assert!(matches!(
        check_admission(&c.state, &sa),
        Err(VmError::BudgetExhausted)
    ));
    let (r, _) = c.block(&[sa]);
    assert!(matches!(err(&r[0]), VmError::BudgetExhausted));
    audit_clean(&c.state);
}

#[test]
fn walking_keel_from_key_to_key_buys_no_capacity() {
    // The flood the free grant allowed: one coin, moved to a fresh key
    // whenever the previous key's grant ran out. With capacity coming from
    // the lock, the coin a key locked cannot be walked on: it is tied up
    // for the unlock delay, and what the next key can do is bounded by
    // what it locks itself.
    let (mut state, mut alice, _bob, _v) = setup(0);
    network(&mut state);
    let mut c = Chain::new(state);
    let (r, _) = c.block(&[alice.act(Action::LockBudget { amount: KEEL })]);
    ok(&r[0]);

    let mut first = Actor::new(51);
    let mut second = Actor::new(52);
    let (r, _) = c.block(&[alice.act(transfer(first.addr(), 2 * KEEL))]);
    ok(&r[0]);
    // Holding KEEL grants nothing.
    let (r, _) = c.block(&[first.act(transfer(second.addr(), 2 * KEEL))]);
    assert!(matches!(err(&r[0]), VmError::BudgetExhausted));
    // Locking 1 KEEL lets the key move on only the KEEL it did not lock.
    let (r, _) = c.block(&[first.act(Action::LockBudget { amount: KEEL })]);
    ok(&r[0]);
    let (r, _) = c.block(&[first.act(transfer(second.addr(), 2 * KEEL))]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    let (r, _) = c.block(&[first.act(transfer(second.addr(), KEEL))]);
    ok(&r[0]);
    // The locked coin comes back only after the delay, so the capacity of
    // a walk is the capacity of the KEEL it leaves behind, per day.
    let (r, _) = c.block(&[first.act(Action::UnlockBudget { amount: KEEL })]);
    ok(&r[0]);
    assert_eq!(acct(&c.state, first.addr(), &keel(), "deposit"), 0);
    assert_eq!(
        acct(&c.state, first.addr(), &keel(), "budget_unlocking"),
        KEEL as i128
    );
    let (r, _) = c.block(&[second.act(Action::LockBudget { amount: KEEL })]);
    ok(&r[0]);
    assert_eq!(acct(&c.state, second.addr(), &keel(), "deposit"), 0);
    audit_clean(&c.state);
}
