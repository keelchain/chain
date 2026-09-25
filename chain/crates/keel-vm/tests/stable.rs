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
    // Burn 1 KUSD -> 1e12 wei back.
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
