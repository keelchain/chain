//! What the chain mints after genesis (2026-11-16): the block reward on a
//! four-year halving schedule with a proposer slice, and the community
//! reward per dollar of protocol fee, both capped so supply converges to
//! the cap.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_ledger::{AccountKey, TxType};
use keel_types::{Address, Asset};
use keel_vm::{apply_block, modules::rewards, BlockContext, Event};

const MIN_V: u128 = 100_000_000_000;
const KEEL: u128 = 1_000_000;
const DAY_MS: u64 = 86_400_000;

fn block(c: &mut Chain, ms: u64, proposer: Option<Address>) -> Vec<Event> {
    c.height += 1;
    let ctx = BlockContext {
        height: c.height,
        timestamp: ms,
        proposer,
    };
    apply_block(&mut c.state, &ctx, &[]).1
}

#[test]
fn the_block_reward_follows_the_halving_schedule_and_pays_the_proposer() {
    let (mut state, _alice, _bob, v) = setup(MIN_V);
    // The shared setup switches the rewards off; a devnet genesis has them.
    assert_eq!(
        keel_vm::Genesis::devnet(keel_actions::CHAIN_ID_DEVNET, &[], vec![])
            .build()
            .rewards
            .validator_cap,
        rewards::KEEL_CAP / 10,
        "10% of the cap, by block"
    );
    state.rewards.validator_cap = rewards::KEEL_CAP / 10;
    let mut c = Chain::new(state);
    let cap = c.state.rewards.validator_cap;
    assert_eq!(c.state.rewards.start_ms, 0);
    // The devnet validator's consensus key is its address.
    let proposer = Some(Address(v.addr().0));

    // The first block starts the clock and mints nothing.
    let t0 = 1_700_000_000_000;
    let ev = block(&mut c, t0, proposer);
    assert!(!ev
        .iter()
        .any(|e| matches!(e, Event::BlockRewardMinted { .. })));
    assert_eq!(c.state.rewards.start_ms, t0);
    assert_eq!(rewards::rate_per_year(&c.state, t0), cap / 8);

    // One day later: a day of the first-era rate, a tenth to the proposer,
    // the rest into the pool for the epoch payout.
    let v_before = acct(&c.state, v.addr(), &keel(), "deposit");
    let ev = block(&mut c, t0 + DAY_MS, proposer);
    let expected = (cap / 8) * DAY_MS as u128 / (365 * DAY_MS as u128);
    let to_proposer = expected / 10;
    assert!(ev.contains(&Event::BlockRewardMinted {
        height: c.height,
        amount: expected,
        proposer: Some(v.addr()),
        to_proposer,
    }));
    assert_eq!(
        acct(&c.state, v.addr(), &keel(), "deposit"),
        v_before + to_proposer as i128
    );
    assert_eq!(
        sys(&c.state, &keel(), "validator_rewards"),
        (expected - to_proposer) as i128
    );
    assert_eq!(c.state.rewards.validator_minted, expected);
    assert!(
        expected > 700_000 * KEEL && expected < 740_000 * KEEL,
        "{expected}"
    );

    // An unknown proposer (not a validator): everything to the pool.
    let pool_before = sys(&c.state, &keel(), "validator_rewards");
    let ev = block(&mut c, t0 + 2 * DAY_MS, Some(Address([9u8; 32])));
    assert!(ev.iter().any(|e| matches!(
        e,
        Event::BlockRewardMinted {
            to_proposer: 0,
            proposer: None,
            ..
        }
    )));
    assert_eq!(
        sys(&c.state, &keel(), "validator_rewards"),
        pool_before + expected as i128
    );

    // Halvings: the rate is a constant of the schedule, not a parameter.
    assert_eq!(
        rewards::rate_per_year(&c.state, t0 + rewards::HALVING_MS),
        cap / 16
    );
    assert_eq!(
        rewards::rate_per_year(&c.state, t0 + 3 * rewards::HALVING_MS),
        cap / 64
    );
    assert_eq!(
        rewards::next_halving_ms(&c.state, t0 + DAY_MS),
        Some(t0 + rewards::HALVING_MS)
    );

    // The cap is the ceiling: lower it and the mint clamps to what is left.
    c.state.rewards.validator_cap = c.state.rewards.validator_minted + KEEL;
    let ev = block(&mut c, t0 + 3 * DAY_MS, proposer);
    assert!(ev
        .iter()
        .any(|e| matches!(e, Event::BlockRewardMinted { amount, .. } if *amount == KEEL)));
    let ev = block(&mut c, t0 + 4 * DAY_MS, proposer);
    assert!(!ev
        .iter()
        .any(|e| matches!(e, Event::BlockRewardMinted { .. })));
    assert_eq!(c.state.rewards.validator_remaining(), 0);
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn protocol_fees_mint_the_community_reward_to_the_payer() {
    let (mut state, alice, _bob, _v) = setup(0);
    state.rewards.community_cap = rewards::KEEL_CAP / 4;
    let mut c = Chain::new(state);
    let usdt = usds();
    let before = acct(&c.state, alice.addr(), &keel(), "deposit");

    // A $2 fee paid in the dollar through the fee split mints 20 KEEL to
    // the payer, on top of the split itself.
    keel_vm::modules::fees::collect(
        &mut c.state,
        "fee:test:1",
        TxType::SendoutComplete,
        None,
        AccountKey::new(alice.addr(), usdt.clone(), "deposit").unwrap(),
        &usdt,
        2_000_000,
    )
    .unwrap();
    let ev = block(&mut c, 1_700_000_000_000, None);
    assert!(ev.contains(&Event::CommunityRewardMinted {
        owner: alice.addr(),
        fee_usd: 2_000_000,
        amount: 20 * KEEL,
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        before + (20 * KEEL) as i128
    );
    assert_eq!(c.state.rewards.community_minted, 20 * KEEL);

    // A fee paid in KEEL (client usage) earns nothing; nor does a system
    // payer; and the cap is the ceiling.
    rewards::community_for_fee(&mut c.state, alice.addr(), &keel(), 5 * KEEL, "k");
    rewards::community_for_fee(&mut c.state, Address::SYSTEM, &usdt, 5_000_000, "s");
    assert_eq!(c.state.rewards.community_minted, 20 * KEEL);
    c.state.rewards.community_cap = c.state.rewards.community_minted + KEEL;
    rewards::community_for_fee(&mut c.state, alice.addr(), &usdt, 9_000_000, "cap");
    assert_eq!(c.state.rewards.community_remaining(), 0);
    let _ = Asset::new("KEEL");
    assert!(c.state.ledger.audit().mismatches.is_empty());
}
