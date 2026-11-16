//! Staking emission (2026-11-15): the genesis `validator_bootstrap` bucket
//! pays validators, observers and delegators a governed share per year,
//! accrued at each epoch boundary and paid by the epoch distribution.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::Address;
use keel_vm::{modules::emission, Event, VmError};

const MIN_V: u128 = 100_000_000_000;
const KEEL: u128 = 1_000_000;
const DAY: u64 = 86_400;

fn bucket(state: &mut keel_vm::State, amount: u128) {
    state
        .ledger
        .post(
            "fund:bootstrap",
            TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, keel(), "issuance").unwrap(),
                    amount,
                ),
                Record::credit(
                    AccountKey::new(Address::SYSTEM, keel(), "validator_bootstrap").unwrap(),
                    amount,
                ),
            ],
        )
        .unwrap();
}

#[test]
fn the_bootstrap_bucket_pays_the_epoch_rewards_at_the_governed_rate() {
    let (mut state, alice, bob, v) = setup(MIN_V);
    state.params.epoch_length_blocks = 2;
    bucket(&mut state, 1_000 * KEEL);
    assert_eq!(emission::remaining(&state), 1_000 * KEEL);
    assert_eq!(
        emission::get_param(&state, "emission.validator_bps_per_year"),
        Some(emission::DEFAULT_VALIDATOR_BPS_PER_YEAR as u128)
    );
    let holders = [alice.addr(), bob.addr(), v.addr()];
    let held = |c: &Chain| -> i128 {
        holders
            .iter()
            .map(|a| acct(&c.state, *a, &keel(), "deposit"))
            .sum()
    };
    let mut c = Chain::new(state);
    let before = held(&c);

    // The first epoch boundary only starts the clock.
    let t0 = 1_000;
    c.block_at_secs(t0, &[]);
    let (_, ev) = c.block_at_secs(t0, &[]);
    assert!(!ev.iter().any(|e| matches!(e, Event::EmissionPaid { .. })));
    assert_eq!(emission::remaining(&c.state), 1_000 * KEEL);

    // One day later: 1000 KEEL × 10% × 1/365, moved into the reward pool
    // and paid out by the same epoch's distribution.
    c.block_at_secs(t0 + DAY, &[]);
    let (_, ev) = c.block_at_secs(t0 + DAY, &[]);
    let expected = 1_000 * KEEL * 1_000 * (DAY as u128 * 1_000) / (10_000 * 365 * 86_400_000);
    assert!(expected > 0);
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::EmissionPaid { epoch: 2, amount, remaining }
            if *amount == expected && *remaining == 1_000 * KEEL - expected
    )));
    assert_eq!(emission::remaining(&c.state), 1_000 * KEEL - expected);
    assert_eq!(c.state.emission.paid_total, expected);
    let paid = held(&c) - before;
    assert!(
        paid > 0 && (expected as i128 - paid) < 8,
        "paid {paid}, expected about {expected} (rounding dust stays in the pool)"
    );

    // The rate is a governance parameter: 0 switches the emission off,
    // more than 100% a year is refused.
    c.block_at_secs(t0 + 2 * DAY, &[]);
    assert!(matches!(
        set_param(&mut c.state, "emission.validator_bps_per_year", 20_000),
        Err(VmError::Invalid(_))
    ));
    set_param(&mut c.state, "emission.validator_bps_per_year", 0).expect("param");
    let remaining = emission::remaining(&c.state);
    let (_, ev) = c.block_at_secs(t0 + 2 * DAY, &[]);
    assert!(!ev.iter().any(|e| matches!(e, Event::EmissionPaid { .. })));
    assert_eq!(emission::remaining(&c.state), remaining);
    assert!(c.state.ledger.audit().mismatches.is_empty());
}
