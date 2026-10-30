//! Client-owned custody vaults (docs/models.md, Model A): registration by
//! an attester only, addresses for the client and its attested accounts,
//! deposits credited against the client's own reserve, withdrawals signed
//! off chain and settled per vault, the per-vault reserve check, and the
//! network reserve check leaving custody out.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{
    Action, Chain as ExtChain, CustodySwap, CustodyVaultRegistration, DepositObservation,
    OutboundObservation, PlaceOrder, Proof, SwapSide, Withdraw,
};
use keel_ledger::{Record, TxType};
use keel_types::{Address, Asset, OrderType, Side};
use keel_vm::{
    modules::{custody, vaults::OutboundStatus},
    Event, State, VmError,
};

fn usdt() -> Asset {
    Asset::new("TRON.USDT")
}
fn trx() -> Asset {
    Asset::new("TRON.TRX")
}

fn registration(epoch: u64) -> CustodyVaultRegistration {
    CustodyVaultRegistration {
        chain: ExtChain::Tron,
        epoch,
        public_key: vec![2u8; 33],
        chain_code: Some([7u8; 32]),
        signer_url: "https://signer.example.com".into(),
    }
}

fn obs(asset: Asset, deposit_index: u64, amount: u128, tx: u8) -> DepositObservation {
    DepositObservation {
        chain: ExtChain::Tron,
        asset,
        tx_hash: [tx; 32],
        index: 0,
        deposit_index,
        amount,
        external_height: 100,
        tip_height: 200,
        proof: Proof::None,
    }
}

fn reserve(c: &Chain, custodian: Address, asset: &Asset) -> i128 {
    c.state.custody.reserve(&c.state, custodian, asset)
}

fn liabilities(c: &Chain, custodian: Address, asset: &Asset) -> u128 {
    c.state
        .custody
        .liabilities
        .get(&(custodian, asset.clone()))
        .copied()
        .unwrap_or(0)
}

#[test]
fn custody_vault_lifecycle() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();

    // Only an attester registers a vault, and the key must be usable.
    let (r, _) = c.block(&[bob.act(Action::RegisterCustodyVault(registration(1)))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    let bad = CustodyVaultRegistration {
        signer_url: "ftp://nope".into(),
        ..registration(1)
    };
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(bad))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(registration(1)))]);
    ok(&r[0]);
    assert!(c.state.custody.vault(ExtChain::Tron, client).is_some());
    // Rotation needs a higher epoch.
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(registration(1)))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // The client vouches for alice; bob is nobody's.
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[
        alice.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
        }),
        bob.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
        }),
        v.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
        }),
        alice.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
        }),
    ]);
    ok(&r[0]);
    assert!(matches!(err(&r[1]), VmError::Unauthorized));
    ok(&r[2]);
    ok(&r[3]);
    assert!(r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::CustodyAddressAssigned { index: 1, .. })));
    assert!(r[2]
        .events
        .iter()
        .any(|e| matches!(e, Event::CustodyAddressAssigned { index: 2, .. })));
    // Asking again returns the same index.
    assert!(r[3]
        .events
        .iter()
        .any(|e| matches!(e, Event::CustodyAddressAssigned { index: 1, .. })));
    assert_eq!(
        c.state.custody.custodian_of.get(&alice.addr()),
        Some(&client)
    );

    // The validator is the devnet's observer: two votes are not needed
    // with a threshold of one. A deposit to alice's address is credited
    // as custody, backed by the client's reserve, not the network's.
    let net_reserve_before = c.state.ledger.system_reserves(&usdt());
    let (r, _) = c.block(&[v.act(Action::ObserveCustodyDeposit {
        custodian: client,
        observation: obs(usdt(), 1, 5_000_000, 1),
    })]);
    ok(&r[0]);
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::CustodyDepositCredited {
            amount: 5_000_000,
            ..
        }
    )));
    assert_eq!(custody::balance(&c.state, alice.addr(), &usdt()), 5_000_000);
    assert_eq!(acct(&c.state, alice.addr(), &usdt(), "deposit"), 0);
    assert_eq!(reserve(&c, client, &usdt()), 5_000_000);
    assert_eq!(liabilities(&c, client, &usdt()), 5_000_000);
    // The network's own reserve check saw nothing of it.
    assert_eq!(
        c.state.ledger.system_reserves(&usdt()) - net_reserve_before,
        5_000_000,
        "the ledger sum includes the client reserve"
    );
    assert!(!c.state.vaults.halted.contains(&usdt()));
    // Same deposit twice is refused; an unknown index too.
    let (r, _) = c.block(&[
        v.act(Action::ObserveCustodyDeposit {
            custodian: client,
            observation: obs(usdt(), 1, 5_000_000, 1),
        }),
        v.act(Action::ObserveCustodyDeposit {
            custodian: client,
            observation: obs(usdt(), 9, 1, 2),
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::NotFound(_)));

    // A custody balance cannot leave through the network vault.
    let (r, _) = c.block(&[alice.act(Action::Withdraw(Withdraw {
        asset: usdt(),
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount: 1_000_000,
    }))]);
    assert!(!r[0].ok);
    // It leaves through the client's vault: escrowed, batched per client.
    c.state.params.outbound_batch_interval_blocks = 1;
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustody(Withdraw {
        asset: usdt(),
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount: 2_000_000,
    }))]);
    ok(&r[0]);
    let id = r[0]
        .events
        .iter()
        .find_map(|e| match e {
            Event::CustodyWithdrawalQueued { outbound_id, .. } => Some(*outbound_id),
            _ => None,
        })
        .unwrap();
    assert_eq!(custody::balance(&c.state, alice.addr(), &usdt()), 3_000_000);
    assert_eq!(
        acct(&c.state, alice.addr(), &usdt(), "custody_escrow"),
        2_000_000
    );
    let ob = &c.state.vaults.outbounds[&id];
    assert_eq!(ob.status, OutboundStatus::Batched);
    let batch = ob.batch_id.unwrap();
    assert_eq!(c.state.custody.batch_vault.get(&batch), Some(&client));
    assert_eq!(c.state.custody.outbound_vault.get(&id), Some(&client));

    // Confirmed on chain with a 1 TRX fee the client has no gas for: the
    // amount leaves the reserve, the fee is booked as the client's
    // expense and its TRX reserve is now short, which halts TRX outbounds.
    let (r, end) = c.block(&[v.act(Action::ObserveOutbound(OutboundObservation {
        outbound_id: id,
        tx_hash: [9; 32],
        external_height: 300,
        tip_height: 310,
        fee_paid: 1_000_000,
        success: true,
    }))]);
    ok(&r[0]);
    assert_eq!(
        c.state.vaults.outbounds[&id].status,
        OutboundStatus::Confirmed
    );
    assert_eq!(acct(&c.state, alice.addr(), &usdt(), "custody_escrow"), 0);
    assert_eq!(reserve(&c, client, &usdt()), 3_000_000);
    assert_eq!(liabilities(&c, client, &usdt()), 3_000_000);
    assert_eq!(reserve(&c, client, &trx()), -1_000_000);
    assert!(end
        .iter()
        .any(|e| matches!(e, Event::CustodyReserveBreached { asset, .. } if *asset == trx())));
    assert!(c.state.custody.halted.contains(&(client, trx())));
    assert!(!c.state.custody.halted.contains(&(client, usdt())));
    assert!(c.state.ledger.audit().mismatches.is_empty());

    // The client tops up gas at its own address (index 0): the fee it
    // owed is settled from the top-up, the reserve covers its balance
    // again and the halt clears on its own.
    let (r, end) = c.block(&[v.act(Action::ObserveCustodyDeposit {
        custodian: client,
        observation: obs(trx(), 0, 3_000_000, 3),
    })]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, client, &trx()), 2_000_000);
    assert_eq!(reserve(&c, client, &trx()), 2_000_000);
    assert_eq!(liabilities(&c, client, &trx()), 2_000_000);
    assert_eq!(acct(&c.state, client, &trx(), "sendout_network_fee"), 0);
    assert!(!end
        .iter()
        .any(|e| matches!(e, Event::CustodyReserveBreached { .. })));
    assert!(!c.state.custody.halted.contains(&(client, trx())));

    // A failed outbound refunds the custody balance; the next fee comes
    // out of the client's gas balance, so the TRX reserve stays covered.
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustody(Withdraw {
        asset: usdt(),
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount: 1_000_000,
    }))]);
    ok(&r[0]);
    let id2 = id + 1;
    let (r, _) = c.block(&[v.act(Action::ObserveOutbound(OutboundObservation {
        outbound_id: id2,
        tx_hash: [10; 32],
        external_height: 320,
        tip_height: 330,
        fee_paid: 0,
        success: false,
    }))]);
    ok(&r[0]);
    assert_eq!(
        c.state.vaults.outbounds[&id2].status,
        OutboundStatus::Failed
    );
    assert_eq!(custody::balance(&c.state, alice.addr(), &usdt()), 3_000_000);
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustody(Withdraw {
        asset: usdt(),
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount: 1_000_000,
    }))]);
    ok(&r[0]);
    let id3 = id2 + 1;
    let (r, end) = c.block(&[v.act(Action::ObserveOutbound(OutboundObservation {
        outbound_id: id3,
        tx_hash: [11; 32],
        external_height: 340,
        tip_height: 350,
        fee_paid: 500_000,
        success: true,
    }))]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, client, &trx()), 1_500_000);
    assert_eq!(reserve(&c, client, &trx()), 1_500_000);
    assert_eq!(liabilities(&c, client, &trx()), 1_500_000);
    assert!(!end
        .iter()
        .any(|e| matches!(e, Event::CustodyReserveBreached { .. })));
    assert!(c.state.custody.halted.is_empty());
    assert_eq!(reserve(&c, client, &usdt()), 2_000_000);
    assert_eq!(liabilities(&c, client, &usdt()), 2_000_000);
    assert!(c.state.ledger.audit().mismatches.is_empty());

    // Totals per asset match what the network check subtracts.
    let (res, liab) = custody::totals(&c.state, &usdt());
    assert_eq!(res, 2_000_000);
    assert_eq!(liab, 2_000_000);
}

#[test]
fn custody_accounts_belong_to_one_client() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    // bob is a second attester with his own vault.
    state.attest.attesters.insert(bob.addr());
    let mut c = Chain::new(state);
    let (r, _) = c.block(&[
        v.act(Action::RegisterCustodyVault(registration(1))),
        bob.act(Action::RegisterCustodyVault(registration(1))),
        v.act(Action::Attest {
            subject: alice.addr(),
            tier: 1,
            expires_at: T0 + 1_000_000,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    ok(&r[2]);
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Tron,
        custodian: v.addr(),
    })]);
    ok(&r[0]);
    // Re-attested by bob later, alice still cannot open custody with him:
    // her balances are backed by one vault.
    let (r, _) = c.block(&[bob.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Tron,
        custodian: bob.addr(),
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // Deposits into the two vaults never mix.
    let (r, _) = c.block(&[
        v.act(Action::ObserveCustodyDeposit {
            custodian: v.addr(),
            observation: obs(usdt(), 1, 1_000_000, 1),
        }),
        v.act(Action::ObserveCustodyDeposit {
            custodian: bob.addr(),
            observation: obs(usdt(), 0, 4_000_000, 2),
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(reserve(&c, v.addr(), &usdt()), 1_000_000);
    assert_eq!(reserve(&c, bob.addr(), &usdt()), 4_000_000);
    assert_eq!(custody::balance(&c.state, bob.addr(), &usdt()), 4_000_000);
    let (res, liab) = custody::totals(&c.state, &usdt());
    assert_eq!((res, liab), (5_000_000, 5_000_000));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

/// A deposit the observers credited to `owner` in `custodian`'s vault,
/// without the proof dance: reserve -> custody, liabilities up.
fn fund_custody(state: &mut State, custodian: Address, owner: Address, asset: &Asset, amount: u128) {
    state
        .ledger
        .post(
            &format!("fund:custody:{}:{}", owner.to_hex(), amount),
            TxType::VaultDeposit,
            None,
            None,
            vec![
                Record::debit(custody::reserve_key(custodian, asset), amount),
                Record::credit(custody::custody_key(owner, asset), amount),
            ],
        )
        .unwrap();
    *state
        .custody
        .liabilities
        .entry((custodian, asset.clone()))
        .or_insert(0) += amount;
}

fn swap(asset: Asset, amount: u128, to: SwapSide, on_behalf_of: Option<Address>) -> Action {
    Action::SwapCustody(CustodySwap {
        asset,
        amount,
        to,
        on_behalf_of,
    })
}

/// The float swap (docs/models.md): a custody balance becomes a
/// network-backed `deposit` balance against the custodian's own float, in
/// one posting with no external transaction, trades on the book, and comes
/// back the same way. Neither reserve check moves.
#[test]
fn swap_between_custody_and_the_market() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    let reg = CustodyVaultRegistration {
        chain: ExtChain::Bitcoin,
        ..registration(1)
    };
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(reg))]);
    ok(&r[0]);
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Bitcoin,
        custodian: client,
    })]);
    ok(&r[0]);
    // Alice holds 1 BTC in the client's vault; the client holds its genesis
    // 100 BTC `deposit` float on the network side and no custody float yet.
    fund_custody(&mut c.state, client, alice.addr(), &btc(), 100_000_000);
    let alice_dep0 = acct(&c.state, alice.addr(), &btc(), "deposit");
    let client_dep0 = acct(&c.state, client, &btc(), "deposit");
    let net_reserve = c.state.ledger.system_reserves(&btc());
    let net_liab = c.state.ledger.user_liabilities(&btc());

    // Refusals: no custody (bob), zero, more than held, nobody's behalf.
    let (r, _) = c.block(&[
        bob.act(swap(btc(), 1, SwapSide::Deposit, None)),
        alice.act(swap(btc(), 0, SwapSide::Deposit, None)),
        alice.act(swap(btc(), 200_000_000, SwapSide::Deposit, None)),
        bob.act(swap(btc(), 1, SwapSide::Deposit, Some(alice.addr()))),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    assert!(matches!(err(&r[2]), VmError::NotEnoughFunds));
    assert!(matches!(err(&r[3]), VmError::Unauthorized));
    // Back towards custody needs the client's custody float, which is empty.
    let (r, _) = c.block(&[alice.act(swap(btc(), 1, SwapSide::Custody, None))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("float")));

    // 0.6 BTC enters the market.
    let (r, _) = c.block(&[alice.act(swap(btc(), 60_000_000, SwapSide::Deposit, None))]);
    ok(&r[0]);
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::CustodySwapped { amount: 60_000_000, to, .. } if to == "deposit"
    )));
    assert_eq!(custody::balance(&c.state, alice.addr(), &btc()), 40_000_000);
    assert_eq!(custody::balance(&c.state, client, &btc()), 60_000_000);
    assert_eq!(
        acct(&c.state, alice.addr(), &btc(), "deposit"),
        alice_dep0 + 60_000_000
    );
    assert_eq!(acct(&c.state, client, &btc(), "deposit"), client_dep0 - 60_000_000);
    // The custody vault backs the same total; so does the network vault.
    assert_eq!(reserve(&c, client, &btc()), 100_000_000);
    assert_eq!(liabilities(&c, client, &btc()), 100_000_000);
    assert_eq!(c.state.ledger.system_reserves(&btc()), net_reserve);
    assert_eq!(c.state.ledger.user_liabilities(&btc()), net_liab);
    assert!(c.state.custody.halted.is_empty());
    assert!(c.state.ledger.audit().mismatches.is_empty());

    // It trades on the chain's book like any wallet user's balance.
    let (r, _) = c.block(&[
        alice.act(Action::PlaceOrder(PlaceOrder {
            pair: "BTC-KUSD".into(),
            side: Side::Sell,
            order_type: OrderType::Limit,
            price: Some(60_000_000_000),
            quantity: Some(30_000_000),
            quote_budget: None,
            client_id: None,
        })),
        bob.act(Action::PlaceOrder(PlaceOrder {
            pair: "BTC-KUSD".into(),
            side: Side::Buy,
            order_type: OrderType::Limit,
            price: Some(60_010_000_000),
            quantity: Some(30_000_000),
            quote_budget: None,
            client_id: None,
        })),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(
        acct(&c.state, alice.addr(), &btc(), "deposit"),
        alice_dep0 + 30_000_000
    );
    assert!(acct(&c.state, alice.addr(), &usds(), "deposit") > 1_000_000_000_000);
    // The fill's taker fee left user balances; the swap back must not.
    let net_liab = c.state.ledger.user_liabilities(&btc());

    // The custodian brings 0.2 BTC back into its own vault for alice.
    let (r, _) = c.block(&[v.act(swap(
        btc(),
        20_000_000,
        SwapSide::Custody,
        Some(alice.addr()),
    ))]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, alice.addr(), &btc()), 60_000_000);
    assert_eq!(custody::balance(&c.state, client, &btc()), 40_000_000);
    assert_eq!(
        acct(&c.state, alice.addr(), &btc(), "deposit"),
        alice_dep0 + 10_000_000
    );
    assert_eq!(acct(&c.state, client, &btc(), "deposit"), client_dep0 - 40_000_000);
    assert_eq!(reserve(&c, client, &btc()), 100_000_000);
    assert_eq!(liabilities(&c, client, &btc()), 100_000_000);
    assert_eq!(c.state.ledger.system_reserves(&btc()), net_reserve);
    assert_eq!(c.state.ledger.user_liabilities(&btc()), net_liab);

    // A halted vault freezes swaps in both directions.
    c.state.custody.halted.insert((client, btc()));
    let (r, _) = c.block(&[
        alice.act(swap(btc(), 1, SwapSide::Deposit, None)),
        alice.act(swap(btc(), 1, SwapSide::Custody, None)),
    ]);
    assert!(matches!(err(&r[0]), VmError::Paused(_)));
    assert!(matches!(err(&r[1]), VmError::Paused(_)));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

/// A wallet user the client attested can join its custody through a swap
/// without ever asking for a custody address.
#[test]
fn swap_into_custody_enrols_an_attested_account() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    let reg = CustodyVaultRegistration {
        chain: ExtChain::Bitcoin,
        ..registration(1)
    };
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(reg))]);
    ok(&r[0]);
    // The client's custody float: its own top-up at index 0.
    fund_custody(&mut c.state, client, client, &btc(), 50_000_000);
    // Not attested: no custodian to swap against.
    let (r, _) = c.block(&[alice.act(swap(btc(), 10_000_000, SwapSide::Custody, None))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    let dep0 = acct(&c.state, alice.addr(), &btc(), "deposit");
    let (r, _) = c.block(&[alice.act(swap(btc(), 10_000_000, SwapSide::Custody, None))]);
    ok(&r[0]);
    assert_eq!(c.state.custody.custodian_of.get(&alice.addr()), Some(&client));
    assert_eq!(custody::balance(&c.state, alice.addr(), &btc()), 10_000_000);
    assert_eq!(custody::balance(&c.state, client, &btc()), 40_000_000);
    assert_eq!(acct(&c.state, alice.addr(), &btc(), "deposit"), dep0 - 10_000_000);
    assert_eq!(reserve(&c, client, &btc()), 50_000_000);
    assert_eq!(liabilities(&c, client, &btc()), 50_000_000);
    // Bob was never attested: still nobody's.
    let (r, _) = c.block(&[bob.act(swap(btc(), 1, SwapSide::Custody, None))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}
