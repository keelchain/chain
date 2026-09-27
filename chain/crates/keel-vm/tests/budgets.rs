//! Action capacity by locking KEEL (2026-09-08, the Tron energy model).

mod common;

use common::*;
use keel_actions::Action;
use keel_vm::{Event, VmError};

const KEEL: u128 = 1_000_000;

#[test]
fn locked_keel_grants_capacity_and_comes_back_after_the_delay() {
    let (mut state, mut alice, _bob, _v) = setup(0);
    // Tiny free budget so the lock is what carries alice.
    state.params.budget.base = 2;
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

    // The lock action itself spent 1 of the 2 free actions. One more free,
    // then the pool: after 12 h it holds 500.
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
        "free budget gone, pool still empty"
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
