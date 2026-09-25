#![allow(clippy::unwrap_used)]
//! Governance: proposal lifecycle, tally rules, execution of every kind.

mod common;

use common::*;
use keel_actions::{Action, Bond, PlaceOrder, Proposal, ProposalKind, Role, VoteChoice};
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Asset, OrderType, Side};
use keel_vm::{modules::gov::ProposalStatus, Event, VmError};

const MIN_V: u128 = 100_000_000_000;
const DEPOSIT: u128 = 1_000_000_000;

fn proposal(kind: ProposalKind) -> Action {
    Action::Propose(Proposal {
        title: "t".into(),
        description: "d".into(),
        kind,
    })
}

const ALICE: usize = 0;
const BOB: usize = 1;
const V: usize = 2;

/// Genesis validator (bond MIN_V) + alice bonded MIN_V: two equal voters.
/// Short voting period and timelock. Actors: [alice, bob, validator].
fn gov_setup() -> (Chain, Vec<Actor>) {
    let (state, mut alice, bob, validator) = setup(MIN_V);
    let mut c = Chain::new(state);
    c.state.params.proposal_deposit = DEPOSIT;
    c.state.params.voting_period_blocks = 5;
    c.state.params.timelock_blocks = 2;
    let (r, _) = c.block(&[alice.act(Action::Bond(Bond {
        role: Role::Validator,
        amount: MIN_V,
        consensus_key: Some(alice.addr().0),
    }))]);
    ok(&r[0]);
    (c, vec![alice, bob, validator])
}

fn status(c: &Chain, id: u64) -> ProposalStatus {
    c.state.gov.proposals[&id].status
}

/// Propose at the next block, apply `votes`, run the tally, wait out the
/// timelock and execute. Returns the proposal id and the execute receipt.
fn run(
    c: &mut Chain,
    actors: &mut [Actor],
    proposer: usize,
    kind: ProposalKind,
    votes: &[(usize, VoteChoice)],
) -> (u64, keel_vm::Receipt) {
    let id = c.state.gov.next_id;
    let (r, _) = c.block(&[actors[proposer].act(proposal(kind))]);
    ok(&r[0]);
    let p = c.state.gov.proposals[&id].clone();
    for (voter, choice) in votes {
        let (r, _) = c.block(&[actors[*voter].act(Action::Vote {
            proposal_id: id,
            choice: *choice,
        })]);
        ok(&r[0]);
    }
    c.advance_to(p.voting_end);
    c.advance_to(p.timelock_end);
    let (r, _) = c.block(&[actors[proposer].act(Action::ExecuteProposal { proposal_id: id })]);
    (id, r.into_iter().next().unwrap())
}

const YES: &[(usize, VoteChoice)] = &[(ALICE, VoteChoice::Yes)];

#[test]
fn param_change_lifecycle_with_deposit_timelock_and_events() {
    let (mut c, mut actors) = gov_setup();
    let alice = &mut actors[ALICE];
    let keel_before = acct(&c.state, alice.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[alice.act(proposal(ProposalKind::ParamChange {
        key: "taker_fee_bps".into(),
        value: 25,
    }))]);
    ok(&r[0]);
    assert!(r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::ProposalCreated { proposal_id: 0, .. })));
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        keel_before - DEPOSIT as i128
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "proposal_deposit"),
        DEPOSIT as i128
    );
    let p = c.state.gov.proposals[&0].clone();
    assert_eq!(p.voting_end, c.height + 5);
    assert_eq!(p.timelock_end, c.height + 7);

    // Unknown params are refused at proposal time.
    let (r, _) = c.block(&[alice.act(proposal(ProposalKind::ParamChange {
        key: "nope".into(),
        value: 1,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // Voting: weight is bonded KEEL; unbonded accounts cannot vote; re-vote replaces.
    let mut stranger = Actor::new(33);
    let (r, _) = c.block(&[
        alice.act(Action::Vote {
            proposal_id: 0,
            choice: VoteChoice::No,
        }),
        alice.act(Action::Vote {
            proposal_id: 0,
            choice: VoteChoice::Yes,
        }),
        stranger.act(Action::Vote {
            proposal_id: 0,
            choice: VoteChoice::Yes,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert!(matches!(err(&r[2]), VmError::Invalid(_)));
    assert_eq!(c.state.gov.proposals[&0].yes, MIN_V);
    assert_eq!(c.state.gov.proposals[&0].no, 0);

    // Executing before the tally / timelock is refused.
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: 0 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let events = c.advance_to(p.voting_end);
    assert!(events.iter().any(
        |e| matches!(e, Event::ProposalTallied { proposal_id: 0, status } if status == "passed")
    ));
    assert_eq!(status(&c, 0), ProposalStatus::Passed);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        keel_before,
        "deposit refunded on pass"
    );
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: 0 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)), "timelock");
    // Voting after close is refused.
    let (r, _) = c.block(&[alice.act(Action::Vote {
        proposal_id: 0,
        choice: VoteChoice::Yes,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    c.advance_to(p.timelock_end);
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: 0 })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::ParamChanged {
        key: "taker_fee_bps".into(),
        value: 25
    }));
    assert!(r[0].events.contains(&Event::ProposalExecuted {
        proposal_id: 0,
        ok: true
    }));
    assert_eq!(c.state.params.taker_fee_bps, 25);
    assert_eq!(status(&c, 0), ProposalStatus::Executed);
    // Executing twice is refused.
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: 0 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    audit_clean(&c.state);
}

#[test]
fn quorum_failure_rejects_and_refunds() {
    let (mut c, mut actors) = gov_setup();
    let (a, rest) = actors.split_at_mut(1);
    let alice = &mut a[0];
    let v = &mut rest[1];
    // total bonded 2*MIN_V, quorum 33.4% -> needs > 66.8e9; nobody votes.
    let (r, _) = c.block(&[alice.act(proposal(ProposalKind::Text))]);
    ok(&r[0]);
    let p = c.state.gov.proposals[&0].clone();
    let keel_after_deposit = acct(&c.state, alice.addr(), &keel(), "deposit");
    c.advance_to(p.voting_end);
    assert_eq!(status(&c, 0), ProposalStatus::Rejected);
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        keel_after_deposit + DEPOSIT as i128
    );
    assert_eq!(acct(&c.state, alice.addr(), &keel(), "proposal_deposit"), 0);
    // A rejected proposal cannot be executed.
    c.advance_to(p.timelock_end);
    let (r, _) = c.block(&[alice.act(Action::ExecuteProposal { proposal_id: 0 })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // Majority No: rejected as well.
    let (r, _) = c.block(&[alice.act(proposal(ProposalKind::Text))]);
    ok(&r[0]);
    let (r, _) = c.block(&[
        v.act(Action::Vote {
            proposal_id: 1,
            choice: VoteChoice::No,
        }),
        alice.act(Action::Vote {
            proposal_id: 1,
            choice: VoteChoice::Abstain,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    let p = c.state.gov.proposals[&1].clone();
    c.advance_to(p.voting_end);
    assert_eq!(status(&c, 1), ProposalStatus::Rejected);
    audit_clean(&c.state);
}

#[test]
fn veto_burns_the_deposit() {
    let (mut c, mut actors) = gov_setup();
    let (a, rest) = actors.split_at_mut(1);
    let alice = &mut a[0];
    let v = &mut rest[1];
    let burn_before = sys(&c.state, &keel(), "burn");
    let (r, _) = c.block(&[alice.act(proposal(ProposalKind::Text))]);
    ok(&r[0]);
    let (r, _) = c.block(&[
        alice.act(Action::Vote {
            proposal_id: 0,
            choice: VoteChoice::Yes,
        }),
        v.act(Action::Vote {
            proposal_id: 0,
            choice: VoteChoice::Veto,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    let p = c.state.gov.proposals[&0].clone();
    let events = c.advance_to(p.voting_end);
    assert!(events.iter().any(
        |e| matches!(e, Event::ProposalTallied { proposal_id: 0, status } if status == "vetoed")
    ));
    assert_eq!(status(&c, 0), ProposalStatus::Vetoed);
    assert_eq!(
        sys(&c.state, &keel(), "burn"),
        burn_before + DEPOSIT as i128
    );
    assert_eq!(acct(&c.state, alice.addr(), &keel(), "proposal_deposit"), 0);
    audit_clean(&c.state);
}

#[test]
fn bad_fee_split_fails_at_execution_and_leaves_params() {
    let (mut c, mut actors) = gov_setup();
    let (id, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::ParamChange {
            key: "fee_split_burn_bps".into(),
            value: 5_000,
        },
        &[],
    );
    // With no votes it is rejected; vote first.
    assert_eq!(status(&c, id), ProposalStatus::Rejected);
    assert!(!r.ok);
    let (id, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::ParamChange {
            key: "fee_split_burn_bps".into(),
            value: 5_000,
        },
        YES,
    );
    ok(&r);
    assert!(r.events.contains(&Event::ProposalExecuted {
        proposal_id: id,
        ok: false
    }));
    assert_eq!(status(&c, id), ProposalStatus::Failed);
    assert_eq!(c.state.params.fee_split_burn_bps, 1_000);
    assert!(c.state.params.fee_split_ok());
    audit_clean(&c.state);
}

#[test]
fn treasury_spend_respects_reserves_over_liabilities() {
    let (mut c, mut actors) = gov_setup();
    let bob = actors[BOB].addr();
    // Reserves: vault_asset BTC 3e10 (three funded accounts) = liabilities.
    // Give the treasury 1e9 out of reserves (reserves 3.1e10), then leak
    // 5e8 of reserves into an unrestricted system pocket (reserves 3.05e10),
    // so only 5e8 of the treasury is actually surplus.
    c.state
        .ledger
        .post(
            "seed:treasury",
            TxType::SystemFundsDeposit,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, btc(), "vault_asset").unwrap(),
                    1_000_000_000,
                ),
                Record::credit(
                    AccountKey::new(Address::SYSTEM, btc(), "treasury").unwrap(),
                    1_000_000_000,
                ),
            ],
        )
        .unwrap();
    c.state
        .ledger
        .post(
            "seed:leak",
            TxType::SystemFundsExpense,
            None,
            None,
            vec![
                Record::debit(
                    AccountKey::new(Address::SYSTEM, btc(), "system_funds").unwrap(),
                    500_000_000,
                ),
                Record::credit(
                    AccountKey::new(Address::SYSTEM, btc(), "vault_asset").unwrap(),
                    500_000_000,
                ),
            ],
        )
        .unwrap();
    let reserves = c.state.ledger.system_reserves(&btc()) as u128;
    let liabilities = c.state.ledger.user_liabilities(&btc());
    assert_eq!(reserves - liabilities, 500_000_000);
    let bob_before = acct(&c.state, bob, &btc(), "deposit");

    // Spending the whole treasury would leave reserves short: refused.
    let (id, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::TreasurySpend {
            to: bob,
            asset: btc(),
            amount: 1_000_000_000,
        },
        YES,
    );
    ok(&r);
    assert_eq!(status(&c, id), ProposalStatus::Failed);
    assert_eq!(acct(&c.state, bob, &btc(), "deposit"), bob_before);
    assert_eq!(sys(&c.state, &btc(), "treasury"), 1_000_000_000);
    // Spending exactly the surplus is fine.
    let (id, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::TreasurySpend {
            to: bob,
            asset: btc(),
            amount: 500_000_000,
        },
        YES,
    );
    ok(&r);
    assert_eq!(status(&c, id), ProposalStatus::Executed);
    assert_eq!(
        acct(&c.state, bob, &btc(), "deposit"),
        bob_before + 500_000_000
    );
    assert_eq!(sys(&c.state, &btc(), "treasury"), 500_000_000);
    // Native KEEL has no reserve invariant: the treasury can pay out anything it holds.
    fund_system(&mut c.state, &keel(), "treasury", 7, "keel-treasury");
    let (id, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::TreasurySpend {
            to: bob,
            asset: keel(),
            amount: 7,
        },
        YES,
    );
    ok(&r);
    assert_eq!(status(&c, id), ProposalStatus::Executed);
    audit_clean(&c.state);
}

#[test]
fn membership_basket_pause_upgrade_and_listing_kinds() {
    let (mut c, mut actors) = gov_setup();
    let (alice_addr, bob_addr, v_addr) =
        (actors[ALICE].addr(), actors[BOB].addr(), actors[V].addr());

    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetArbitrators {
            members: vec![bob_addr],
        },
        YES,
    );
    ok(&r);
    assert_eq!(
        keel_vm::modules::staking::arbitrators(&c.state),
        vec![bob_addr]
    );

    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetObservers {
            members: vec![bob_addr, v_addr],
            threshold: 2,
        },
        YES,
    );
    ok(&r);
    let (members, threshold) = keel_vm::modules::staking::observers(&c.state);
    assert_eq!(members.len(), 2);
    assert!(members.contains(&bob_addr) && members.contains(&v_addr));
    assert_eq!(threshold, 2);
    // A threshold above the member count is refused at proposal time.
    let (r, _) = c.block(&[actors[ALICE].act(proposal(ProposalKind::SetObservers {
        members: vec![bob_addr],
        threshold: 2,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetAttesters {
            members: vec![alice_addr],
        },
        YES,
    );
    ok(&r);
    assert!(c.state.attest.attesters.contains(&alice_addr));
    assert_eq!(c.state.attest.attesters.len(), 1);

    let eth_usdt = Asset::new("ETH.USDT");
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetStableBasket {
            asset: eth_usdt.clone(),
            cap: 5,
            enabled: false,
        },
        YES,
    );
    ok(&r);
    let e = &c.state.stable.basket[&eth_usdt];
    assert_eq!((e.cap, e.enabled), (5, false));
    // The stable itself can never be a reserve asset.
    let (r, _) = c.block(&[actors[ALICE].act(proposal(ProposalKind::SetStableBasket {
        asset: usds(),
        cap: 1,
        enabled: true,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::RegisterAsset {
            asset: Asset::new("ETH.USDC"),
            decimals: 6,
        },
        YES,
    );
    ok(&r);
    assert!(c.state.tokens.is_registered(&Asset::new("ETH.USDC")));
    let (r, _) = c.block(&[actors[ALICE].act(proposal(ProposalKind::RegisterAsset {
        asset: usds(),
        decimals: 6,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    let cfg =
        keel_vm::modules::markets::default_pair("USDC-KUSD", Asset::new("ETH.USDC"), usds(), 6, 6);
    let (_, r) = run(&mut c, &mut actors, ALICE, ProposalKind::ListPair(cfg), YES);
    ok(&r);
    assert!(c.state.markets.pairs.contains_key("USDC-KUSD"));
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::DelistPair {
            symbol: "USDC-KUSD".into(),
        },
        YES,
    );
    ok(&r);
    assert!(!c.state.markets.pairs["USDC-KUSD"].cfg.enabled);

    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SoftwareUpgrade {
            version: "v0.2.0".into(),
            height: 1_000_000,
        },
        YES,
    );
    ok(&r);
    assert_eq!(
        c.state.gov.upgrades,
        vec![("v0.2.0".to_string(), 1_000_000)]
    );
    let (r, _) = c.block(&[actors[ALICE].act(proposal(ProposalKind::SoftwareUpgrade {
        version: "v0".into(),
        height: 1,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // Pausing markets makes every markets action fail with Paused until the height.
    let until = c.height + 200;
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::PauseModule {
            module: "markets".into(),
            until_height: until,
        },
        YES,
    );
    ok(&r);
    let alice = &mut actors[ALICE];
    let order = Action::PlaceOrder(PlaceOrder {
        pair: "BTC-KUSD".into(),
        side: Side::Sell,
        order_type: OrderType::Limit,
        price: Some(60_000_000_000),
        quantity: Some(1_000_000),
        quote_budget: None,
        client_id: None,
    });
    let (r, _) = c.block(&[alice.act(order.clone())]);
    assert_eq!(err(&r[0]), &VmError::Paused("markets".into()));
    c.advance_to(until);
    let (r, _) = c.block(&[alice.act(order)]);
    ok(&r[0]);
    audit_clean(&c.state);
}

#[test]
fn devnet_house_operator_via_param_change() {
    let (mut c, mut actors) = gov_setup();
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::ParamChange {
            key: "house_operator_seed".into(),
            value: 21,
        },
        YES,
    );
    ok(&r);
    assert_eq!(c.state.gov_house_operator, Some(actors[ALICE].addr()));
    let alice = &mut actors[ALICE];
    // Not available off devnet.
    c.state.chain_id = 2;
    let (r, _) = c.block(&[alice.act(proposal(ProposalKind::ParamChange {
        key: "house_operator_seed".into(),
        value: 1,
    }))]);
    assert!(matches!(
        err(&r[0]),
        VmError::WrongChain | VmError::Invalid(_)
    ));
    audit_clean(&c.state);
}

#[test]
fn light_client_checkpoints_and_token_contracts_are_governed() {
    use keel_actions::{BtcCheckpoint, EthCheckpoint};
    let (mut c, mut actors) = gov_setup();
    // Bitcoin regtest genesis header (height 0), PoW valid at regtest difficulty.
    let genesis_header = hex::decode("0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4adae5494dffff7f2002000000").unwrap();
    assert!(c.state.vaults.btc_chain.is_none());
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetBtcCheckpoint(BtcCheckpoint {
            network: 3,
            height: 0,
            header: genesis_header.clone(),
            period_start_time: 1_296_688_602,
        }),
        YES,
    );
    ok(&r);
    assert_eq!(
        c.state
            .vaults
            .btc_chain
            .as_ref()
            .map(|h| h.checkpoint_height),
        Some(0)
    );
    // A header that fails its own proof of work is refused at execution and
    // leaves the chain untouched (regtest's target is trivial, so break the
    // hash by raising `bits` to an impossible target instead of flipping a
    // version byte).
    let mut bad = genesis_header.clone();
    bad[72..76].copy_from_slice(&0x1d00_ffffu32.to_le_bytes()); // mainnet difficulty
                                                                // Execution failures are reported as `ProposalExecuted { ok: false }`
                                                                // on a successful action, and the proposal ends up Failed.
    let (id, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetBtcCheckpoint(BtcCheckpoint {
            network: 3,
            height: 5,
            header: bad,
            period_start_time: 0,
        }),
        YES,
    );
    ok(&r);
    assert!(r.events.contains(&Event::ProposalExecuted {
        proposal_id: id,
        ok: false
    }));
    assert_eq!(status(&c, id), ProposalStatus::Failed);
    assert_eq!(
        c.state
            .vaults
            .btc_chain
            .as_ref()
            .map(|h| h.checkpoint_height),
        Some(0)
    );
    // Ethereum bootstrap.
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetEthCheckpoint(EthCheckpoint {
            period: 900,
            committee_root: [1; 32],
            next_committee_root: Some([2; 32]),
            genesis_validators_root: [3; 32],
            fork_version: [4, 0, 0, 0],
            committee_size: 512,
        }),
        YES,
    );
    ok(&r);
    let sync = c.state.vaults.eth_sync.as_ref().unwrap();
    assert_eq!(
        (
            sync.period,
            sync.committee_root,
            sync.next_committee_root,
            sync.committee_size
        ),
        (900, [1; 32], Some([2; 32]), 512)
    );
    // Token contract: 20 bytes for a registered vault asset only.
    let (_, r) = run(
        &mut c,
        &mut actors,
        ALICE,
        ProposalKind::SetTokenContract {
            asset: Asset::new("ETH.USDT"),
            contract: vec![0xda; 20],
        },
        YES,
    );
    ok(&r);
    assert_eq!(
        c.state.vaults.token_contracts[&Asset::new("ETH.USDT")],
        vec![0xda; 20]
    );
    for kind in [
        ProposalKind::SetTokenContract {
            asset: Asset::new("ETH.USDT"),
            contract: vec![1; 19],
        },
        ProposalKind::SetTokenContract {
            asset: Asset::new("ETH.NOPE"),
            contract: vec![1; 20],
        },
    ] {
        let (id, r) = run(&mut c, &mut actors, ALICE, kind, YES);
        ok(&r);
        assert_eq!(status(&c, id), ProposalStatus::Failed);
    }
    assert_eq!(
        c.state.vaults.token_contracts[&Asset::new("ETH.USDT")],
        vec![0xda; 20]
    );
    audit_clean(&c.state);
}
