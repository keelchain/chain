#![allow(clippy::unwrap_used)]
//! Staking: bonds, delegation, unbonding queue, validator set, epochs,
//! rewards, slashing.

mod common;

use common::*;
use keel_actions::{Action, Bond, Role};
use keel_vm::{modules::staking, Event, VmError};

const MIN_V: u128 = 100_000_000_000; // params.min_validator_bond

fn bond(role: Role, amount: u128, key: Option<[u8; 32]>) -> Action {
    Action::Bond(Bond {
        role,
        amount,
        consensus_key: key,
    })
}

#[test]
fn bond_delegate_unbond_and_release() {
    let (state, mut alice, mut bob, validator) = setup(2 * MIN_V);
    let mut c = Chain::new(state);
    c.state.params.unbonding_blocks = 5;

    // Genesis validator holds its bond in stake_bond, issued at genesis.
    assert_eq!(
        acct(&c.state, validator.addr(), &keel(), "stake_bond"),
        (2 * MIN_V) as i128
    );
    assert_eq!(staking::total_bonded(&c.state), 2 * MIN_V);

    // Below the minimum: refused. A new validator needs a consensus key.
    let (r, _) = c.block(&[
        alice.act(bond(Role::Validator, MIN_V / 2, Some(alice.addr().0))),
        alice.act(bond(Role::Validator, MIN_V, None)),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    assert!(!c.state.staking.validators.contains_key(&alice.addr()));

    // Alice bonds; Bob delegates 30% of a bond to her.
    let (r, _) = c.block(&[
        alice.act(bond(Role::Validator, MIN_V, Some(alice.addr().0))),
        bob.act(Action::Delegate {
            validator: alice.addr(),
            amount: 30_000_000_000,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        (1_000_000_000_000u128 - MIN_V) as i128
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "stake_bond"),
        MIN_V as i128
    );
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "stake_bond"),
        30_000_000_000
    );
    let v = &c.state.staking.validators[&alice.addr()];
    assert_eq!(v.self_bond, MIN_V);
    assert_eq!(v.delegated, 30_000_000_000);
    assert_eq!(
        staking::voting_weight(&c.state, &alice.addr()),
        MIN_V + 30_000_000_000
    );
    assert_eq!(staking::voting_weight(&c.state, &bob.addr()), 0);
    assert_eq!(staking::total_bonded(&c.state), 3 * MIN_V + 30_000_000_000);

    // Validator set: power-ordered, genesis validator first.
    let set = staking::validator_set(&c.state);
    assert_eq!(set.len(), 2);
    assert_eq!(set[0], (validator.addr().0, 2 * MIN_V));
    assert_eq!(set[1], (alice.addr().0, MIN_V + 30_000_000_000));
    c.state.params.max_validators = 1;
    assert_eq!(staking::validator_set(&c.state).len(), 1);
    c.state.params.max_validators = 100;

    // Self-delegation and delegating to a non-validator are refused.
    let (r, _) = c.block(&[
        alice.act(Action::Delegate {
            validator: alice.addr(),
            amount: 1,
        }),
        bob.act(Action::Delegate {
            validator: bob.addr(),
            amount: 1,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));

    // Partial unbond that would leave a sub-minimum bond is refused;
    // undelegate too much is refused; undelegate part is queued.
    let h = c.height + 1;
    let (r, _) = c.block(&[
        alice.act(Action::Unbond {
            role: Role::Validator,
            amount: 1,
        }),
        bob.act(Action::Undelegate {
            validator: alice.addr(),
            amount: 40_000_000_000,
        }),
        bob.act(Action::Undelegate {
            validator: alice.addr(),
            amount: 10_000_000_000,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    ok(&r[2]);
    let release = h + 5;
    assert!(r[2].events.iter().any(|e| matches!(e, Event::Undelegated { at_height, amount: 10_000_000_000, .. } if *at_height == release)));
    assert_eq!(
        c.state.staking.validators[&alice.addr()].delegated,
        20_000_000_000
    );
    // Still in the bond account until release.
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "stake_bond"),
        30_000_000_000
    );

    // Alice unbonds everything: validator stays (delegations remain) with zero self bond.
    let (r, _) = c.block(&[alice.act(Action::Unbond {
        role: Role::Validator,
        amount: MIN_V,
    })]);
    ok(&r[0]);
    assert_eq!(c.state.staking.validators[&alice.addr()].self_bond, 0);
    assert_eq!(
        staking::voting_weight(&c.state, &alice.addr()),
        20_000_000_000
    );

    // Release after the unbonding period.
    let events = c.advance_to(release);
    assert!(events.iter().any(|e| matches!(e, Event::BondReleased { owner, amount: 10_000_000_000, .. } if *owner == bob.addr())));
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "stake_bond"),
        20_000_000_000
    );
    assert_eq!(
        acct(&c.state, bob.addr(), &keel(), "deposit"),
        1_000_000_000_000i128 - 20_000_000_000
    );
    let events = c.advance_to(release + 1);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::BondReleased { owner, .. } if *owner == alice.addr())));
    assert_eq!(acct(&c.state, alice.addr(), &keel(), "stake_bond"), 0);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        1_000_000_000_000
    );
    assert!(c.state.staking.unbonding.is_empty());
    audit_clean(&c.state);
}

#[test]
fn observer_and_arbitrator_bonds_and_membership() {
    let (state, mut alice, _bob, validator) = setup(MIN_V);
    let mut c = Chain::new(state);
    // Genesis seeds membership from the validators; alice is neither.
    assert!(staking::is_observer(&c.state, &validator.addr()));
    assert!(staking::is_arbitrator(&c.state, &validator.addr()));
    assert!(!staking::is_observer(&c.state, &alice.addr()));
    assert_eq!(staking::observers(&c.state), (vec![validator.addr()], 1));
    // Bonds move to the role's restricted account and count for votes.
    let (r, _) = c.block(&[
        alice.act(bond(Role::Observer, c.state.params.min_observer_bond, None)),
        alice.act(bond(
            Role::Arbitrator,
            c.state.params.min_arbitrator_bond,
            None,
        )),
        alice.act(bond(Role::Arbitrator, 1, None)),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    ok(&r[2]);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "observer_bond"),
        c.state.params.min_observer_bond as i128
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "arbitrator_bond"),
        c.state.params.min_arbitrator_bond as i128 + 1
    );
    assert_eq!(
        staking::voting_weight(&c.state, &alice.addr()),
        c.state.params.min_observer_bond + c.state.params.min_arbitrator_bond + 1
    );
    // Membership is governance's call, not the bond's.
    assert!(!staking::is_observer(&c.state, &alice.addr()));
    staking::set_observers(&mut c.state, &[alice.addr(), validator.addr()], 2);
    staking::set_arbitrators(&mut c.state, &[alice.addr()]);
    assert!(staking::is_observer(&c.state, &alice.addr()));
    assert_eq!(staking::observers(&c.state).1, 2);
    assert_eq!(staking::arbitrators(&c.state), vec![alice.addr()]);
    assert!(!staking::is_arbitrator(&c.state, &validator.addr()));
    // ClaimRewards is not a thing: rewards are auto-distributed.
    let (r, _) = c.block(&[alice.act(Action::ClaimRewards)]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    audit_clean(&c.state);
}

#[test]
fn epoch_distributes_rewards_to_observers_validators_and_delegators() {
    let (state, mut alice, mut bob, validator) = setup(2 * MIN_V);
    let mut c = Chain::new(state);
    c.state.params.epoch_length_blocks = 10;
    let (r, _) = c.block(&[
        alice.act(bond(Role::Validator, MIN_V, Some(alice.addr().0))),
        bob.act(Action::Delegate {
            validator: alice.addr(),
            amount: 30_000_000_000,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    // 1 KUSD of fees accrued to the pool.
    fund_system(
        &mut c.state,
        &usds(),
        "validator_rewards",
        1_000_000,
        "rewards",
    );
    let before = |c: &Chain, who| acct(&c.state, who, &usds(), "deposit");
    let (v0, a0, b0) = (
        before(&c, validator.addr()),
        before(&c, alice.addr()),
        before(&c, bob.addr()),
    );

    let events = c.advance_to(10);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::EpochAdvanced {
            epoch: 1,
            validators: 2
        }
    )));
    assert_eq!(c.state.staking.epoch, 1);
    // Observer share 25% -> the genesis validator (sole observer): 250_000.
    // Validator share 750_000 over power 3.3e11: V 454_545, alice 295_454 of
    // which bob's 3e10/1.3e11 slice is 68_181, alice keeps 227_273.
    assert_eq!(before(&c, validator.addr()) - v0, 250_000 + 454_545);
    assert_eq!(before(&c, alice.addr()) - a0, 227_273);
    assert_eq!(before(&c, bob.addr()) - b0, 68_181);
    assert_eq!(sys(&c.state, &usds(), "validator_rewards"), 1); // rounding dust stays
    assert!(events.iter().any(|e| matches!(
        e,
        Event::RewardsDistributed {
            epoch: 1,
            amount: 999_999,
            ..
        }
    )));
    // No epoch before the next boundary; an empty pool distributes nothing.
    let events = c.advance_to(19);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::EpochAdvanced { .. })));
    let events = c.advance_to(20);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::EpochAdvanced { epoch: 2, .. })));
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::RewardsDistributed { .. })));
    audit_clean(&c.state);
}

#[test]
fn slash_burns_bonds_and_jails() {
    let (state, mut alice, _bob, _validator) = setup(MIN_V);
    let mut c = Chain::new(state);
    let (r, _) = c.block(&[
        alice.act(bond(Role::Validator, MIN_V, Some(alice.addr().0))),
        alice.act(bond(
            Role::Arbitrator,
            c.state.params.min_arbitrator_bond,
            None,
        )),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(staking::validator_set(&c.state).len(), 2);
    let burned = staking::slash(&mut c.state, &alice.addr(), 1_000, "double sign");
    let expected = MIN_V / 10 + c.state.params.min_arbitrator_bond / 10;
    assert_eq!(burned, expected);
    assert_eq!(sys(&c.state, &keel(), "burn"), expected as i128);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "stake_bond"),
        (MIN_V - MIN_V / 10) as i128
    );
    let v = &c.state.staking.validators[&alice.addr()];
    assert!(v.jailed);
    assert_eq!(v.self_bond, MIN_V - MIN_V / 10);
    assert_eq!(v.power(), 0);
    assert_eq!(staking::validator_set(&c.state).len(), 1);
    // Nobody can delegate to a jailed validator.
    let mut bob = Actor::new(22);
    let (r, _) = c.block(&[bob.act(Action::Delegate {
        validator: alice.addr(),
        amount: 1_000,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // Slashing a stranger burns nothing.
    assert_eq!(
        staking::slash(&mut c.state, &Actor::new(99).addr(), 1_000, "nothing"),
        0
    );
    audit_clean(&c.state);
}
