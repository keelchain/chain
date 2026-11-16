#![allow(clippy::unwrap_used)]
//! The USD stablecoin: mint/burn against the basket, caps, decimals,
//! reserve invariant.

mod common;

use common::*;
use keel_actions::Action;
use keel_types::Asset;
use keel_vm::{modules::stable, Event, VmError};

fn eth_usdt() -> Asset {
    Asset::new("ETH.USDT")
}
fn eth() -> Asset {
    Asset::new("ETH.ETH")
}

fn invariant(state: &keel_vm::State) {
    assert!(
        state.stable.supply <= stable::reserves_in_stable(state),
        "supply exceeds reserves"
    );
    assert_eq!(
        sys(state, &usds(), "issuance"),
        (1_000_000_000_000 * 3 + state.stable.supply) as i128,
        "issuance tracks genesis + supply"
    );
    for (asset, e) in &state.stable.basket {
        assert_eq!(sys(state, asset, "stable_reserve"), e.reserve as i128);
    }
    audit_clean(state);
}

#[test]
fn mint_and_burn_round_trip_same_decimals() {
    let (state, mut alice, _bob, _v) = setup(0);
    let mut c = Chain::new(state);
    fund_vault_asset(&mut c.state, alice.addr(), &eth_usdt(), 5_000_000, "usdt");
    let usds_before = acct(&c.state, alice.addr(), &usds(), "deposit");
    let (r, _) = c.block(&[alice.act(Action::MintStable {
        asset: eth_usdt(),
        amount: 1_000_000,
    })]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::StableMinted {
        owner: alice.addr(),
        from: eth_usdt(),
        amount: 1_000_000
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        usds_before + 1_000_000
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &eth_usdt(), "deposit"),
        4_000_000
    );
    assert_eq!(c.state.stable.basket[&eth_usdt()].reserve, 1_000_000);
    assert_eq!(c.state.stable.supply, 1_000_000);
    invariant(&c.state);

    let (r, _) = c.block(&[alice.act(Action::BurnStable {
        asset: eth_usdt(),
        amount: 400_000,
    })]);
    ok(&r[0]);
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        usds_before + 600_000
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &eth_usdt(), "deposit"),
        4_400_000
    );
    assert_eq!(c.state.stable.basket[&eth_usdt()].reserve, 600_000);
    assert_eq!(c.state.stable.supply, 600_000);
    invariant(&c.state);

    // Cannot redeem more than this asset's reserve, nor mint/burn zero or
    // against an asset outside the basket.
    let (r, _) = c.block(&[
        alice.act(Action::BurnStable {
            asset: eth_usdt(),
            amount: 600_001,
        }),
        alice.act(Action::MintStable {
            asset: eth_usdt(),
            amount: 0,
        }),
        alice.act(Action::MintStable {
            asset: btc(),
            amount: 1,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    assert!(matches!(err(&r[2]), VmError::NotFound(_)));
    // Minting more than the wallet holds fails atomically.
    let (r, _) = c.block(&[alice.act(Action::MintStable {
        asset: eth_usdt(),
        amount: 4_400_001,
    })]);
    assert_eq!(err(&r[0]), &VmError::NotEnoughFunds);
    assert_eq!(c.state.stable.supply, 600_000);
    invariant(&c.state);
}

#[test]
fn caps_and_disabled_assets_are_enforced() {
    let (state, mut alice, _bob, _v) = setup(0);
    let mut c = Chain::new(state);
    fund_vault_asset(&mut c.state, alice.addr(), &eth_usdt(), 10_000_000, "usdt");
    stable::set_basket(&mut c.state, eth_usdt(), 2_000_000, true);
    let (r, _) = c.block(&[
        alice.act(Action::MintStable {
            asset: eth_usdt(),
            amount: 1_500_000,
        }),
        alice.act(Action::MintStable {
            asset: eth_usdt(),
            amount: 500_001,
        }),
        alice.act(Action::MintStable {
            asset: eth_usdt(),
            amount: 500_000,
        }),
    ]);
    ok(&r[0]);
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    ok(&r[2]);
    assert_eq!(c.state.stable.basket[&eth_usdt()].reserve, 2_000_000);
    // Disabled: no more minting, burning still allowed so holders can exit.
    stable::set_basket(&mut c.state, eth_usdt(), 2_000_000, false);
    let (r, _) = c.block(&[
        alice.act(Action::MintStable {
            asset: eth_usdt(),
            amount: 1,
        }),
        alice.act(Action::BurnStable {
            asset: eth_usdt(),
            amount: 2_000_000,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    ok(&r[1]);
    assert_eq!(c.state.stable.supply, 0);
    assert_eq!(c.state.stable.basket[&eth_usdt()].reserve, 0);
    invariant(&c.state);
}

#[test]
fn decimals_convert_between_reserve_and_stable() {
    let (state, mut alice, _bob, _v) = setup(0);
    let mut c = Chain::new(state);
    // ETH.ETH has 18 decimals; 1.5 ETH treated as a $1.5 reserve (a unit
    // test of the conversion path, not a price claim).
    stable::set_basket(&mut c.state, eth(), u128::MAX / 2, true);
    fund_vault_asset(
        &mut c.state,
        alice.addr(),
        &eth(),
        5 * 10u128.pow(18),
        "eth",
    );
    let usds_before = acct(&c.state, alice.addr(), &usds(), "deposit");
    let (r, _) = c.block(&[alice.act(Action::MintStable {
        asset: eth(),
        amount: 1_500_000_000_000_000_000,
    })]);
    ok(&r[0]);
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        usds_before + 1_500_000
    );
    assert_eq!(c.state.stable.supply, 1_500_000);
    assert_eq!(stable::reserves_in_stable(&c.state), 1_500_000);
    // Dust below one stable unit is refused, never minted unbacked.
    let (r, _) = c.block(&[alice.act(Action::MintStable {
        asset: eth(),
        amount: 999_999_999_999,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // Burn 1 USDT -> 1e12 wei back.
    let (r, _) = c.block(&[alice.act(Action::BurnStable {
        asset: eth(),
        amount: 1_000_000,
    })]);
    ok(&r[0]);
    assert_eq!(
        acct(&c.state, alice.addr(), &eth(), "deposit"),
        (5 * 10u128.pow(18) - 1_500_000_000_000_000_000 + 10u128.pow(18)) as i128
    );
    assert_eq!(
        c.state.stable.basket[&eth()].reserve,
        500_000_000_000_000_000
    );
    assert_eq!(c.state.stable.supply, 500_000);
    invariant(&c.state);
}

#[test]
fn stable_pause_stops_mints() {
    let (state, mut alice, _bob, _v) = setup(0);
    let mut c = Chain::new(state);
    fund_vault_asset(&mut c.state, alice.addr(), &eth_usdt(), 10, "usdt");
    c.state.paused.insert("stable".into(), 10);
    let (r, _) = c.block(&[alice.act(Action::MintStable {
        asset: eth_usdt(),
        amount: 1,
    })]);
    assert_eq!(err(&r[0]), &VmError::Paused("stable".into()));
    c.advance_to(10);
    let (r, _) = c.block(&[alice.act(Action::MintStable {
        asset: eth_usdt(),
        amount: 1,
    })]);
    ok(&r[0]);
    invariant(&c.state);
}

// ---------------- one dollar (2026-11-15): conversion on arrival, one-step
// withdrawal, and rebalancing by premium and rebate.

fn tron_usdt() -> Asset {
    Asset::new("TRON.USDT")
}

fn register_tron(c: &mut Chain, v: &mut Actor) {
    register(c, v, keel_actions::Chain::Tron);
}

fn register(c: &mut Chain, v: &mut Actor, chain: keel_actions::Chain) {
    let (r, _) = c.block(&[
        v.act(Action::RegisterVault(keel_actions::VaultRegistration {
            chain,
            epoch: 1,
            public_key: vec![2u8; 33],
            chain_code: Some([7u8; 32]),
            signers: vec![v.addr()],
            threshold: 1,
        })),
    ]);
    ok(&r[0]);
}

fn deposit_tron_usdt(
    c: &mut Chain,
    v: &mut Actor,
    deposit_index: u64,
    amount: u128,
    tx: u8,
) -> keel_vm::Receipt {
    let (r, _) = c.block(&[
        v.act(Action::ObserveDeposit(keel_actions::DepositObservation {
            external_tx: None,
            chain: keel_actions::Chain::Tron,
            asset: tron_usdt(),
            tx_hash: [tx; 32],
            index: 0,
            deposit_index,
            amount,
            external_height: 100,
            tip_height: 200,
            proof: keel_actions::Proof::None,
        })),
    ]);
    r.into_iter().next().unwrap()
}

#[test]
fn a_usdt_deposit_on_any_network_is_one_usdt_balance_withdrawn_in_one_step() {
    let (state, mut alice, _bob, mut v) = setup(0);
    assert_eq!(state.stable_conversion.params.auto_convert, 1);
    assert!(
        !state.stable.basket.contains_key(&Asset::new("SOL.USDC")),
        "USDC is not USDT: it stays USDC with its own book"
    );
    let mut c = Chain::new(state);
    register_tron(&mut c, &mut v);
    let (r, _) = c.block(&[alice.act(Action::RequestDepositAddress {
        chain: keel_actions::Chain::Tron,
    })]);
    ok(&r[0]);
    let index = r[0]
        .events
        .iter()
        .find_map(|e| match e {
            Event::DepositAddressAssigned { index, .. } => Some(*index),
            _ => None,
        })
        .unwrap();
    let usdt_before = acct(&c.state, alice.addr(), &usds(), "deposit");

    // 10 USDT arrive on Tron: alice holds 10 more USDT, no TRON.USDT.
    let r = deposit_tron_usdt(&mut c, &mut v, index, 10_000_000, 1);
    ok(&r);
    assert!(r.events.contains(&Event::StableMinted {
        owner: alice.addr(),
        from: tron_usdt(),
        amount: 10_000_000
    }));
    assert_eq!(acct(&c.state, alice.addr(), &tron_usdt(), "deposit"), 0);
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        usdt_before + 10_000_000
    );
    assert_eq!(c.state.stable.basket[&tron_usdt()].reserve, 10_000_000);
    invariant(&c.state);

    // Tron holds the whole reserve: no premium to leave through it. The
    // withdrawal spends exactly 5 USDT: the $1 flat fee comes out of it and
    // 4 USDT are queued for the destination; alice never sees TRON.USDT.
    assert_eq!(stable::premium_bps(&c.state, &tron_usdt()), 0);
    let (r, _) = c.block(&[
        alice.act(Action::WithdrawStable(keel_actions::WithdrawStable {
            via: tron_usdt(),
            to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
            amount: 5_000_000,
        })),
    ]);
    ok(&r[0]);
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::StableBurned {
            amount: 5_000_000,
            ..
        }
    )));
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::WithdrawalQueued {
            amount: 4_000_000,
            ..
        }
    )));
    assert!(!r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::StablePremiumPaid { .. })));
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        usdt_before + 5_000_000
    );
    assert_eq!(acct(&c.state, alice.addr(), &tron_usdt(), "deposit"), 0);
    assert_eq!(c.state.stable.basket[&tron_usdt()].reserve, 5_000_000);
    // Through a network that holds nothing: refused, nothing to pay from.
    let (r, _) = c.block(&[
        alice.act(Action::WithdrawStable(keel_actions::WithdrawStable {
            via: eth_usdt(),
            to: "0x000000000000000000000000000000000000dEaD".into(),
            amount: 2_000_000,
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("reserve")));
    invariant(&c.state);

    // Conversion off: the next deposit stays per-network.
    set_param(&mut c.state, "stable.auto_convert", 0).expect("param");
    let r = deposit_tron_usdt(&mut c, &mut v, index, 1_000_000, 2);
    ok(&r);
    assert_eq!(
        acct(&c.state, alice.addr(), &tron_usdt(), "deposit"),
        1_000_000
    );
    invariant(&c.state);
}

#[test]
fn a_short_network_charges_a_premium_and_pays_it_back_as_a_rebate() {
    let (state, mut alice, _bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    register_tron(&mut c, &mut v);
    register(&mut c, &mut v, keel_actions::Chain::Ethereum);
    // Reserves: 10 ETH.USDT and 90 TRON.USDT; three equal caps, so each
    // network's target is a third. Ethereum holds 10%: 70% below target,
    // and with a 1% maximum the premium through it is 69 bps (floored).
    fund_vault_asset(&mut c.state, alice.addr(), &eth_usdt(), 10_000_000, "e");
    fund_vault_asset(&mut c.state, alice.addr(), &tron_usdt(), 90_000_000, "t");
    let (r, _) = c.block(&[
        alice.act(Action::MintStable {
            asset: eth_usdt(),
            amount: 10_000_000,
        }),
        alice.act(Action::MintStable {
            asset: tron_usdt(),
            amount: 90_000_000,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(
        stable::shares_ppm(&c.state, &eth_usdt()),
        Some((333_333, 100_000))
    );
    assert_eq!(stable::premium_bps(&c.state, &eth_usdt()), 69);
    assert_eq!(stable::premium_bps(&c.state, &tron_usdt()), 0);

    // Withdraw 5 USDT through Ethereum: 0.0345 USDT premium into the pool.
    let usdt_before = acct(&c.state, alice.addr(), &usds(), "deposit");
    let (r, _) = c.block(&[
        alice.act(Action::WithdrawStable(keel_actions::WithdrawStable {
            via: eth_usdt(),
            to: "0x000000000000000000000000000000000000dEaD".into(),
            amount: 5_000_000,
        })),
    ]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::StablePremiumPaid {
        owner: alice.addr(),
        via: eth_usdt(),
        amount: 34_500
    }));
    assert_eq!(stable::rebalance_pool(&c.state), 34_500);
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        usdt_before - 5_034_500
    );
    // Ethereum is shorter still (5 of 95): the premium rises to 84 bps.
    assert_eq!(stable::premium_bps(&c.state, &eth_usdt()), 84);

    // A deposit that refills Ethereum earns the rebate, bounded by the pool.
    // (Ethereum deposits need light-client proofs in tests; the rebate path
    // is the same for every basket asset, so refill through the module.)
    fund_vault_asset(&mut c.state, alice.addr(), &eth_usdt(), 20_000_000, "e2");
    let before = acct(&c.state, alice.addr(), &usds(), "deposit");
    let events = stable::auto_convert(
        &mut c.state,
        alice.addr(),
        &eth_usdt(),
        20_000_000,
        "refill",
    )
    .unwrap();
    assert!(events.contains(&Event::StableRebatePaid {
        owner: alice.addr(),
        from: eth_usdt(),
        amount: 34_500
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &usds(), "deposit"),
        before + 20_000_000 + 34_500
    );
    assert_eq!(stable::rebalance_pool(&c.state), 0);

    // The premium is a governance parameter; 0 turns rebalancing off.
    assert!(matches!(
        set_param(&mut c.state, "stable.rebalance_max_premium_bps", 20_000),
        Err(VmError::Invalid(_))
    ));
    set_param(&mut c.state, "stable.rebalance_max_premium_bps", 0).expect("param");
    assert_eq!(stable::premium_bps(&c.state, &eth_usdt()), 0);
    invariant(&c.state);
}
