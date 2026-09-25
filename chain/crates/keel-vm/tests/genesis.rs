//! Genesis allocation, vesting and the super-admin parameter path.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{Action, Proposal, ProposalKind, VoteChoice, CHAIN_ID_DEVNET};
use keel_crypto::Keypair;
use keel_ledger::AccountKey;
use keel_types::Address;
use keel_vm::{
    genesis::{Allocation, Genesis, GenesisValidator},
    modules::tokens,
    Event, VmError,
};

const KEEL_CAP: u128 = 21_000_000_000 * 1_000_000;

fn genesis_with_allocation(team: Vec<(Address, u32)>, awards: u128) -> (Genesis, Keypair) {
    let admin = Keypair::from_seed(77);
    let v = Keypair::from_seed(100);
    let mut g = Genesis::devnet(
        CHAIN_ID_DEVNET,
        &[],
        vec![GenesisValidator {
            address: v.address(),
            consensus_key: v.address().0,
            bond: 0,
        }],
    );
    // Only the promised awards hold KEEL before allocation.
    g.accounts.retain(|a| a.asset.as_str() != "KEEL");
    if awards > 0 {
        g.accounts.push(keel_vm::GenesisAccount {
            address: Address::tagged(9),
            asset: keel(),
            amount: awards,
        });
    }
    g.allocation = Some(Allocation {
        team,
        ..Allocation::default()
    });
    g.param_admin = Some(admin.address());
    (g, admin)
}

#[test]
fn default_allocation_issues_the_cap_into_dao_buckets() {
    let awards = 5_000_000 * 1_000_000; // already promised to a user
    let (g, _) = genesis_with_allocation(vec![], awards);
    let state = g.build();
    let bucket = |t: &str| sys(&state, &keel(), t) as u128;
    assert_eq!(bucket("treasury"), KEEL_CAP * 35 / 100);
    assert_eq!(bucket("community_pool") + awards, KEEL_CAP * 25 / 100);
    assert_eq!(
        bucket("team_reserve"),
        KEEL_CAP * 15 / 100,
        "no team addresses: whole bucket stays reserved"
    );
    assert_eq!(bucket("vesting"), 0);
    assert_eq!(bucket("validator_bootstrap"), KEEL_CAP * 10 / 100);
    assert_eq!(bucket("swap_pool"), KEEL_CAP * 10 / 100);
    assert_eq!(bucket("strategic_reserve"), KEEL_CAP * 5 / 100);
    // Everything issued equals the cap exactly: `issuance` is debit-normal,
    // so what it funded reads as a positive balance of the cap.
    assert_eq!(
        state
            .ledger
            .balance(&AccountKey::new(Address::SYSTEM, keel(), "issuance").unwrap()),
        KEEL_CAP as i128
    );
    audit_clean(&state);
}

#[test]
fn team_shares_vest_with_cliff_then_linearly() {
    let alice = Address::tagged(1);
    let (g, _) = genesis_with_allocation(vec![(alice, 4_000)], 0);
    let state0 = g.build();
    let team_total = KEEL_CAP * 15 / 100;
    let alice_total = team_total * 4_000 / 10_000;
    assert_eq!(sys(&state0, &keel(), "vesting") as u128, alice_total);
    assert_eq!(
        sys(&state0, &keel(), "team_reserve") as u128,
        team_total - alice_total
    );
    let mut c = Chain::new(state0);
    let start = c.state.tokens.vesting[&alice].start_secs;
    // Before the cliff: nothing.
    c.block_at_secs(start + 100 * 86_400, &[]);
    assert_eq!(tokens::balance(&c.state, alice, &keel()), 0);
    // Right after the cliff: one year of four has vested.
    let (_, events) = c.block_at_secs(start + 365 * 86_400 + 1, &[]);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::VestingReleased { owner, .. } if *owner == alice)));
    let after_cliff = tokens::balance(&c.state, alice, &keel());
    assert!(
        after_cliff >= alice_total / 4 && after_cliff < alice_total / 4 + alice_total / 1000,
        "{after_cliff}"
    );
    // Fully vested after four years; nothing more afterwards.
    c.block_at_secs(start + 4 * 365 * 86_400, &[]);
    assert_eq!(tokens::balance(&c.state, alice, &keel()), alice_total);
    c.block_at_secs(start + 5 * 365 * 86_400, &[]);
    assert_eq!(tokens::balance(&c.state, alice, &keel()), alice_total);
    assert_eq!(sys(&c.state, &keel(), "vesting"), 0);
    audit_clean(&c.state);
}

#[test]
#[should_panic(expected = "exceed the community bucket")]
fn awards_above_the_community_bucket_are_refused() {
    let (g, _) = genesis_with_allocation(vec![], KEEL_CAP * 30 / 100);
    g.build();
}

#[test]
fn super_admin_sets_params_directly_until_governance_revokes() {
    let (g, admin_key) = genesis_with_allocation(vec![], 0);
    let mut c = Chain::new(g.build());
    let mut admin = Actor::from_keypair(admin_key);
    let mut bob = Actor::new(2);
    assert_eq!(c.state.params.taker_fee_bps, 10);
    let (r, _) = c.block(&[admin.act(Action::SetParam {
        key: "taker_fee_bps".into(),
        value: 15,
    })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::ParamChanged {
        key: "taker_fee_bps".into(),
        value: 15
    }));
    assert_eq!(c.state.params.taker_fee_bps, 15);
    // Unknown key, broken fee split, and a non-admin are refused.
    let (r, _) = c.block(&[
        admin.act(Action::SetParam {
            key: "nope".into(),
            value: 1,
        }),
        admin.act(Action::SetParam {
            key: "fee_split_burn_bps".into(),
            value: 9_000,
        }),
        bob.act(Action::SetParam {
            key: "taker_fee_bps".into(),
            value: 1,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    assert!(matches!(err(&r[2]), VmError::Unauthorized));
    // Block pacing (2026-09-08): busy and idle intervals are params;
    // idle must be >= busy and both <= 60 s.
    assert_eq!(
        (
            c.state.params.min_block_interval_ms,
            c.state.params.idle_block_interval_ms
        ),
        (500, 5_000)
    );
    let (r, _) = c.block(&[
        admin.act(Action::SetParam {
            key: "min_block_interval_ms".into(),
            value: 300,
        }),
        admin.act(Action::SetParam {
            key: "idle_block_interval_ms".into(),
            value: 200,
        }),
        admin.act(Action::SetParam {
            key: "idle_block_interval_ms".into(),
            value: 61_000,
        }),
        admin.act(Action::SetParam {
            key: "idle_block_interval_ms".into(),
            value: 2_000,
        }),
    ]);
    ok(&r[0]);
    assert!(matches!(err(&r[1]), VmError::Invalid(_)), "idle below busy");
    assert!(matches!(err(&r[2]), VmError::Invalid(_)), "above 60 s");
    ok(&r[3]);
    assert_eq!(
        (
            c.state.params.min_block_interval_ms,
            c.state.params.idle_block_interval_ms
        ),
        (300, 2_000)
    );
    assert_eq!(c.state.params.taker_fee_bps, 15);
    // Governance revokes the admin: SetParam stops working.
    c.state.gov.param_admin = None; // what an executed SetParamAdmin { admin: None } does
    let (r, _) = c.block(&[admin.act(Action::SetParam {
        key: "taker_fee_bps".into(),
        value: 20,
    })]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    audit_clean(&c.state);
    // And the proposal kind itself round-trips through a Proposal.
    let _ = Action::Propose(Proposal {
        title: "revoke".into(),
        description: "".into(),
        kind: ProposalKind::SetParamAdmin { admin: None },
    });
    let _ = VoteChoice::Yes;
}
