//! Genesis allocation, vesting and the super-admin parameter path.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{VoteChoice, CHAIN_ID_DEVNET};
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
    (g, admin)
}

#[test]
fn default_allocation_issues_what_genesis_owns_and_caps_the_rest() {
    let awards = 5_000_000 * 1_000_000; // already promised to a user
    let (g, _) = genesis_with_allocation(vec![], awards);
    let state = g.build();
    let bucket = |t: &str| sys(&state, &keel(), t) as u128;
    // Issued at genesis: treasury (the former reserve folded in), team,
    // liquidity, and the awards already promised.
    assert_eq!(bucket("treasury"), KEEL_CAP * 40 / 100);
    assert_eq!(
        bucket("team_reserve"),
        KEEL_CAP * 15 / 100,
        "no team addresses: whole bucket stays reserved"
    );
    assert_eq!(bucket("vesting"), 0);
    assert_eq!(bucket("swap_pool"), KEEL_CAP * 10 / 100);
    // Minted over time, capped: validators by block, the community by usage
    // (less what the awards took at genesis).
    assert_eq!(state.rewards.validator_cap, KEEL_CAP * 10 / 100);
    assert_eq!(state.rewards.community_cap, KEEL_CAP * 25 / 100 - awards);
    assert_eq!(state.rewards.validator_minted, 0);
    // What genesis issued: 65% plus the awards; the rest is minted later,
    // so issued plus both caps is exactly the cap.
    let issued = state
        .ledger
        .balance(&AccountKey::new(Address::SYSTEM, keel(), "issuance").unwrap())
        as u128;
    assert_eq!(issued, KEEL_CAP * 65 / 100 + awards);
    assert_eq!(
        issued + state.rewards.validator_cap + state.rewards.community_cap,
        KEEL_CAP
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
fn executed_parameter_changes_are_validated() {
    let (g, _) = genesis_with_allocation(vec![], 0);
    let mut c = Chain::new(g.build());
    assert_eq!(c.state.params.taker_fee_bps, 10);

    // What an executed `ParamChange` applies, with its validation: unknown
    // keys, a broken fee split, and block pacing out of bounds are refused.
    set_param(&mut c.state, "taker_fee_bps", 15).expect("param");
    assert_eq!(c.state.params.taker_fee_bps, 15);
    assert!(matches!(
        set_param(&mut c.state, "nope", 1),
        Err(VmError::Invalid(_))
    ));
    assert!(matches!(
        set_param(&mut c.state, "fee_split_burn_bps", 9_000),
        Err(VmError::Invalid(_))
    ));
    assert_eq!(
        (
            c.state.params.min_block_interval_ms,
            c.state.params.idle_block_interval_ms
        ),
        (500, 5_000)
    );
    set_param(&mut c.state, "min_block_interval_ms", 300).expect("param");
    assert!(
        matches!(
            set_param(&mut c.state, "idle_block_interval_ms", 200),
            Err(VmError::Invalid(_))
        ),
        "idle below busy"
    );
    assert!(
        matches!(
            set_param(&mut c.state, "idle_block_interval_ms", 61_000),
            Err(VmError::Invalid(_))
        ),
        "above 60 s"
    );
    set_param(&mut c.state, "idle_block_interval_ms", 2_000).expect("param");
    assert_eq!(
        (
            c.state.params.min_block_interval_ms,
            c.state.params.idle_block_interval_ms
        ),
        (300, 2_000)
    );
    audit_clean(&c.state);
    let _ = VoteChoice::Yes;
}
