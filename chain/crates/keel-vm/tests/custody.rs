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
        external_tx: None,
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
            owner: None,
        }),
        bob.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
            owner: None,
        }),
        v.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
            owner: None,
        }),
        alice.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
            owner: None,
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
    assert_eq!(custody::balance(&c.state, alice.addr(), &usds()), 5_000_000);
    // One USDT: the Tron USDT stays in the client's vault as backing and
    // alice holds the unified USDT, as she would in the network vault.
    assert!(r[0].events.contains(&Event::CustodyStableMinted {
        custodian: client,
        owner: alice.addr(),
        asset: usdt(),
        amount_in: 5_000_000,
        minted: 5_000_000,
    }));
    assert_eq!(
        acct(&c.state, alice.addr(), &usdt(), "custody"),
        0,
        "no per-network balance is left on the account"
    );
    assert_eq!(custody::stable_backing(&c.state, client), 5_000_000);
    assert_eq!(liabilities(&c, client, &usds()), 5_000_000);
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
    // USDT names the network it leaves on; the plain custody withdrawal
    // has no network to pay it on.
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustody(Withdraw {
        asset: usds(),
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount: 1_000_000,
    }))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // It leaves through the client's vault: redeemed into Tron USDT,
    // escrowed, batched per client.
    c.state.params.outbound_batch_interval_blocks = 1;
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustodyStable(
        keel_actions::WithdrawStable {
            via: usdt(),
            to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
            amount: 2_000_000,
        },
    ))]);
    ok(&r[0]);
    let id = r[0]
        .events
        .iter()
        .find_map(|e| match e {
            Event::CustodyWithdrawalQueued { outbound_id, .. } => Some(*outbound_id),
            _ => None,
        })
        .unwrap();
    assert_eq!(custody::balance(&c.state, alice.addr(), &usds()), 3_000_000);
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
        external_tx: None,
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
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustodyStable(
        keel_actions::WithdrawStable {
            via: usdt(),
            to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
            amount: 1_000_000,
        },
    ))]);
    ok(&r[0]);
    let id2 = id + 1;
    let (r, _) = c.block(&[v.act(Action::ObserveOutbound(OutboundObservation {
        external_tx: None,
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
    assert_eq!(custody::balance(&c.state, alice.addr(), &usds()), 3_000_000);
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustodyStable(
        keel_actions::WithdrawStable {
            via: usdt(),
            to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
            amount: 1_000_000,
        },
    ))]);
    ok(&r[0]);
    let id3 = id2 + 1;
    let (r, end) = c.block(&[v.act(Action::ObserveOutbound(OutboundObservation {
        external_tx: None,
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
        owner: None,
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
        owner: None,
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
    assert_eq!(custody::balance(&c.state, bob.addr(), &usds()), 4_000_000);
    let (res, liab) = custody::totals(&c.state, &usdt());
    assert_eq!((res, liab), (5_000_000, 5_000_000));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

/// A deposit the observers credited to `owner` in `custodian`'s vault,
/// without the proof dance: reserve -> custody, liabilities up.
fn fund_custody(
    state: &mut State,
    custodian: Address,
    owner: Address,
    asset: &Asset,
    amount: u128,
) {
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
        owner: None,
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
    // An account that never held a custody balance is short of funds,
    // with the code a client can act on.
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
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
    assert_eq!(
        acct(&c.state, client, &btc(), "deposit"),
        client_dep0 - 60_000_000
    );
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
            pair: "BTC-USDT".into(),
            side: Side::Sell,
            order_type: OrderType::Limit,
            price: Some(60_000_000_000),
            quantity: Some(30_000_000),
            quote_budget: None,
            client_id: None,
        })),
        bob.act(Action::PlaceOrder(PlaceOrder {
            pair: "BTC-USDT".into(),
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
    assert_eq!(
        acct(&c.state, client, &btc(), "deposit"),
        client_dep0 - 40_000_000
    );
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
    assert_eq!(
        c.state.custody.custodian_of.get(&alice.addr()),
        Some(&client)
    );
    assert_eq!(custody::balance(&c.state, alice.addr(), &btc()), 10_000_000);
    assert_eq!(custody::balance(&c.state, client, &btc()), 40_000_000);
    assert_eq!(
        acct(&c.state, alice.addr(), &btc(), "deposit"),
        dep0 - 10_000_000
    );
    assert_eq!(reserve(&c, client, &btc()), 50_000_000);
    assert_eq!(liabilities(&c, client, &btc()), 50_000_000);
    // Bob was never attested: still nobody's.
    let (r, _) = c.block(&[bob.act(swap(btc(), 1, SwapSide::Custody, None))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

fn transfer_custody(
    to: Address,
    asset: Asset,
    amount: u128,
    on_behalf_of: Option<Address>,
    memo: &str,
) -> Action {
    Action::TransferCustody(keel_actions::CustodyTransfer {
        legs: vec![keel_actions::CustodyLeg {
            from: None,
            to,
            asset,
            amount,
        }],
        on_behalf_of,
        memo: Some(memo.into()),
    })
}

/// A custodial client posts a trade between two of its users on chain
/// without leaving custody: seller -> the client's escrow account ->
/// buyer, each leg one transfer, with no float and no swap.
#[test]
fn a_custody_balance_moves_between_accounts_of_the_same_client() {
    let (mut state, mut alice, mut bob, mut v) = setup(0);
    // dave is a second client; erin is nobody's user.
    let (dave, erin) = (Actor::new(61), Actor::new(62));
    state.attest.attesters.insert(dave.addr());
    let mut c = Chain::new(state);
    let (r, _) = c.block(&[
        v.act(Action::RegisterCustodyVault(registration(1))),
        v.act(Action::Attest {
            subject: alice.addr(),
            tier: 1,
            expires_at: T0 + 1_000_000,
        }),
        v.act(Action::Attest {
            subject: bob.addr(),
            tier: 1,
            expires_at: T0 + 1_000_000,
        }),
    ]);
    for x in &r {
        ok(x);
    }
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Tron,
        custodian: v.addr(),
        owner: None,
    })]);
    ok(&r[0]);
    fund_custody(&mut c.state, v.addr(), alice.addr(), &usdt(), 10_000_000);
    let backed = (
        reserve(&c, v.addr(), &usdt()),
        liabilities(&c, v.addr(), &usdt()),
    );
    let usage_before = acct(&c.state, v.addr(), &usds(), "deposit");

    // The seller escrows with the client: the client signs for her.
    let (r, _) = c.block(&[v.act(transfer_custody(
        v.addr(),
        usdt(),
        4_000_000,
        Some(alice.addr()),
        "trade:1:escrow",
    ))]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::CustodyTransferred {
        custodian: v.addr(),
        from: alice.addr(),
        to: v.addr(),
        asset: usdt(),
        amount: 4_000_000,
        memo: Some("trade:1:escrow".into()),
    }));
    // Release: escrow -> the buyer, who held no custody balance until now
    // and joins the client's custody by receiving.
    let (r, _) = c.block(&[v.act(transfer_custody(
        bob.addr(),
        usdt(),
        4_000_000,
        None,
        "trade:1:release",
    ))]);
    ok(&r[0]);
    assert_eq!(
        (
            custody::balance(&c.state, alice.addr(), &usdt()),
            custody::balance(&c.state, v.addr(), &usdt()),
            custody::balance(&c.state, bob.addr(), &usdt()),
        ),
        (6_000_000, 0, 4_000_000)
    );
    assert_eq!(
        c.state.custody.custodian_of.get(&bob.addr()),
        Some(&v.addr())
    );
    // Nothing moved on any network: the vault backs the same total, and a
    // transfer is not billed.
    assert_eq!(
        (
            reserve(&c, v.addr(), &usdt()),
            liabilities(&c, v.addr(), &usdt())
        ),
        backed
    );
    assert_eq!(acct(&c.state, v.addr(), &usds(), "deposit"), usage_before);

    // A user moves her own balance to another user of the same client.
    let (r, _) = c.block(&[alice.act(transfer_custody(
        bob.addr(),
        usdt(),
        1_000_000,
        None,
        "gift",
    ))]);
    ok(&r[0]);
    // Nobody but the owner or the client signs for an account.
    let (r, _) = c.block(&[bob.act(transfer_custody(
        bob.addr(),
        usdt(),
        1,
        Some(alice.addr()),
        "theft",
    ))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    // Custody never leaves the client: not to an outsider, not to
    // another client, not for more than is held.
    for to in [erin.addr(), dave.addr()] {
        let (r, _) = c.block(&[alice.act(transfer_custody(to, usdt(), 1, None, "out"))]);
        assert!(matches!(err(&r[0]), VmError::Invalid(_)), "{:?}", r[0]);
    }
    let (r, _) = c.block(&[alice.act(transfer_custody(
        bob.addr(),
        usdt(),
        5_000_001,
        None,
        "too much",
    ))]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));

    // A release with its fee legs is one posting: buyer, fee account and
    // referrer are paid together out of the escrow account.
    let leg = |to: Address, amount: u128| keel_actions::CustodyLeg {
        from: None,
        to,
        asset: usdt(),
        amount,
    };
    let (r, _) = c.block(&[v.act(transfer_custody(
        v.addr(),
        usdt(),
        3_000_000,
        Some(alice.addr()),
        "trade:2:escrow",
    ))]);
    ok(&r[0]);
    let before = (
        custody::balance(&c.state, v.addr(), &usdt()),
        custody::balance(&c.state, bob.addr(), &usdt()),
        custody::balance(&c.state, alice.addr(), &usdt()),
    );
    // One leg the client cannot make (an outsider) refuses the whole
    // release: the buyer is not paid either.
    let (r, _) = c.block(&[
        v.act(Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: vec![leg(bob.addr(), 2_900_000), leg(erin.addr(), 100_000)],
            on_behalf_of: None,
            memo: Some("trade:2:release".into()),
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    // So does a set of legs that together exceed the balance.
    let (r, _) = c.block(&[
        v.act(Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: vec![leg(bob.addr(), 2_900_000), leg(alice.addr(), 100_001)],
            on_behalf_of: None,
            memo: None,
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    assert_eq!(
        (
            custody::balance(&c.state, v.addr(), &usdt()),
            custody::balance(&c.state, bob.addr(), &usdt()),
            custody::balance(&c.state, alice.addr(), &usdt()),
        ),
        before
    );
    let (r, _) = c.block(&[
        v.act(Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: vec![leg(bob.addr(), 2_900_000), leg(alice.addr(), 100_000)],
            on_behalf_of: None,
            memo: Some("trade:2:release".into()),
        })),
    ]);
    ok(&r[0]);
    assert_eq!(r[0].events.len(), 2);
    assert_eq!(
        (
            custody::balance(&c.state, v.addr(), &usdt()),
            custody::balance(&c.state, bob.addr(), &usdt()),
            custody::balance(&c.state, alice.addr(), &usdt()),
        ),
        (
            before.0 - 3_000_000,
            before.1 + 2_900_000,
            before.2 + 100_000
        )
    );
    // Several debits in one posting: alice pays bob, bob pays alice a
    // smaller amount back, and alice pays the client a fee. Only the
    // client can sign for both of them.
    let from = |who: Address, to: Address, amount: u128| keel_actions::CustodyLeg {
        from: Some(who),
        to,
        asset: usdt(),
        amount,
    };
    let (a, b, cl) = (alice.addr(), bob.addr(), v.addr());
    let held = |c: &Chain| {
        (
            custody::balance(&c.state, a, &usdt()),
            custody::balance(&c.state, b, &usdt()),
            custody::balance(&c.state, cl, &usdt()),
        )
    };
    let before = held(&c);
    let exchange = keel_actions::CustodyTransfer {
        legs: vec![
            from(alice.addr(), bob.addr(), 1_000_000),
            from(bob.addr(), alice.addr(), 400_000),
            from(alice.addr(), v.addr(), 50_000),
        ],
        on_behalf_of: None,
        memo: Some("swap:7".into()),
    };
    let (r, _) = c.block(&[alice.act(Action::TransferCustody(exchange.clone()))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    assert_eq!(held(&c), before);
    let (r, _) = c.block(&[v.act(Action::TransferCustody(exchange))]);
    ok(&r[0]);
    assert_eq!(r[0].events.len(), 3);
    assert_eq!(
        held(&c),
        (before.0 - 650_000, before.1 + 600_000, before.2 + 50_000)
    );
    // An account paid and paying in the same transfer needs only the
    // difference: bob passes on more than he held before it.
    let bob_has = custody::balance(&c.state, bob.addr(), &usdt());
    let alice_has = custody::balance(&c.state, alice.addr(), &usdt());
    let (r, _) = c.block(&[
        v.act(Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: vec![
                from(alice.addr(), bob.addr(), alice_has),
                from(bob.addr(), v.addr(), bob_has + alice_has),
            ],
            on_behalf_of: None,
            memo: None,
        })),
    ]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, bob.addr(), &usdt()), 0);
    // One debit too many refuses the whole set.
    let (r, _) = c.block(&[
        v.act(Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: vec![
                from(v.addr(), alice.addr(), 1_000),
                from(bob.addr(), alice.addr(), 1),
            ],
            on_behalf_of: None,
            memo: None,
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    assert_eq!(custody::balance(&c.state, alice.addr(), &usdt()), 0);
    let (r, _) = c.block(&[
        v.act(Action::TransferCustody(keel_actions::CustodyTransfer {
            legs: vec![],
            on_behalf_of: None,
            memo: None,
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

/// One USDT in custody: a client whose vault receives USDT on Tron, BNB
/// Smart Chain and Ethereum gives its users a single USDT balance, and a
/// withdrawal is paid on whichever network the user names, out of what
/// the client's own vault holds there.
#[test]
fn usdt_on_any_network_is_one_custody_balance() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    let (bsc, eth) = (Asset::new("BSC.USDT"), Asset::new("ETH.USDT"));
    let on = |chain: ExtChain| CustodyVaultRegistration {
        chain,
        ..registration(1)
    };
    let mut setup_actions = vec![
        v.act(Action::RegisterCustodyVault(on(ExtChain::Tron))),
        v.act(Action::RegisterCustodyVault(on(ExtChain::Bsc))),
        v.act(Action::RegisterCustodyVault(on(ExtChain::Ethereum))),
    ];
    for who in [&alice, &bob] {
        setup_actions.push(v.act(Action::Attest {
            subject: who.addr(),
            tier: 1,
            expires_at: T0 + 1_000_000,
        }));
    }
    let (r, _) = c.block(&setup_actions);
    for x in &r {
        ok(x);
    }
    let (r, _) = c.block(&[
        alice.act(Action::RequestCustodyAddress {
            chain: ExtChain::Bsc,
            custodian: client,
            owner: None,
        }),
        bob.act(Action::RequestCustodyAddress {
            chain: ExtChain::Ethereum,
            custodian: client,
            owner: None,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    let deposit = |chain: ExtChain, asset: &Asset, index: u64, amount: u128, tx: u8| {
        Action::ObserveCustodyDeposit {
            custodian: client,
            observation: DepositObservation {
                chain,
                ..obs(asset.clone(), index, amount, tx)
            },
        }
    };
    // alice receives 50 USDT on BSC (18 decimals there), bob 30 on
    // Ethereum (ERC-20): each holds plain USDT.
    let bsc_unit = 1_000_000_000_000u128;
    let (r, _) = c.block(&[v.act(deposit(ExtChain::Bsc, &bsc, 1, 50_000_000 * bsc_unit, 21))]);
    ok(&r[0]);
    // An Ethereum deposit is credited on a light-client proof, which this
    // test does not build: credit it and convert it as the chain does.
    fund_custody(&mut c.state, client, bob.addr(), &eth, 30_000_000);
    custody::mint_stable(&mut c.state, client, bob.addr(), &eth, 30_000_000, "eth").unwrap();
    assert_eq!(
        custody::balance(&c.state, alice.addr(), &usds()),
        50_000_000
    );
    assert_eq!(custody::balance(&c.state, bob.addr(), &usds()), 30_000_000);
    assert_eq!(custody::balance(&c.state, alice.addr(), &bsc), 0);
    assert_eq!(custody::stable_backing(&c.state, client), 80_000_000);
    assert_eq!(liabilities(&c, client, &usds()), 80_000_000);

    // It is one currency: alice pays bob in USDT, wherever hers came from.
    let (r, _) = c.block(&[alice.act(transfer_custody(
        bob.addr(),
        usds(),
        10_000_000,
        None,
        "usdt",
    ))]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, bob.addr(), &usds()), 40_000_000);

    // alice, whose USDT arrived on BSC, withdraws on Tron: the client's
    // vault holds no Tron USDT, so that network is refused and nothing is
    // redeemed. She is told why and can pick another.
    let tron_to = "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE";
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustodyStable(
        keel_actions::WithdrawStable {
            via: usdt(),
            to: tron_to.into(),
            amount: 5_000_000,
        },
    ))]);
    assert!(
        matches!(err(&r[0]), VmError::Invalid(m) if m.contains("choose another network")),
        "{:?}",
        r[0]
    );
    assert_eq!(
        custody::balance(&c.state, alice.addr(), &usds()),
        40_000_000
    );
    // The client tops its Tron inventory up at its own address: that too
    // is USDT on its balance, and backing on Tron for everyone.
    let (r, _) = c.block(&[v.act(deposit(ExtChain::Tron, &usdt(), 0, 20_000_000, 23))]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, client, &usds()), 20_000_000);
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustodyStable(
        keel_actions::WithdrawStable {
            via: usdt(),
            to: tron_to.into(),
            amount: 5_000_000,
        },
    ))]);
    ok(&r[0]);
    assert!(r[0].events.contains(&Event::CustodyStableRedeemed {
        custodian: client,
        owner: alice.addr(),
        asset: usdt(),
        burned: 5_000_000,
        amount_out: 5_000_000,
    }));
    assert_eq!(
        custody::balance(&c.state, alice.addr(), &usds()),
        35_000_000
    );
    assert_eq!(
        acct(&c.state, alice.addr(), &usdt(), "custody_escrow"),
        5_000_000
    );
    // bob withdraws on Ethereum, alice on BSC in its own 18 decimals.
    let evm_to = "0x52908400098527886E0F7030069857D2E4169EE7";
    let (r, _) = c.block(&[
        bob.act(Action::WithdrawCustodyStable(
            keel_actions::WithdrawStable {
                via: eth.clone(),
                to: evm_to.into(),
                amount: 30_000_000,
            },
        )),
        alice.act(Action::WithdrawCustodyStable(
            keel_actions::WithdrawStable {
                via: bsc.clone(),
                to: evm_to.into(),
                amount: 1_000_000,
            },
        )),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert_eq!(
        acct(&c.state, alice.addr(), &bsc, "custody_escrow"),
        (1_000_000 * bsc_unit) as i128
    );
    // More than Ethereum holds is refused, though bob has the USDT.
    let (r, _) = c.block(&[bob.act(Action::WithdrawCustodyStable(
        keel_actions::WithdrawStable {
            via: eth.clone(),
            to: evm_to.into(),
            amount: 1,
        },
    ))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));

    // Every USDT in custody is backed, and each network's reserve still
    // covers what is owed on it; nothing is halted.
    let (_, end) = c.block(&[]);
    assert!(!end
        .iter()
        .any(|e| matches!(e, Event::CustodyReserveBreached { .. })));
    assert!(c.state.custody.halted.is_empty());
    assert_eq!(
        custody::stable_backing(&c.state, client),
        liabilities(&c, client, &usds())
    );
    for a in [&usdt(), &bsc, &eth] {
        assert_eq!(
            reserve(&c, client, a),
            liabilities(&c, client, a) as i128,
            "{a}"
        );
    }
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

/// A custodial client needs no key of its users: it attests an account,
/// asks for its deposit address and moves its balance itself. And the
/// service that does this day to day signs with a session key of its own,
/// so it never shares a nonce with the operator key.
#[test]
fn a_client_runs_its_users_accounts_with_a_session_key_and_no_user_keys() {
    let (state, mut alice, _bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    // An account nobody holds a key for: an identifier the client derived.
    let customer = Address::tagged(77);
    let mut bridge = Actor::new(90);
    let now = T0 + c.height + 1;
    let (r, _) = c.block(&[
        v.act(Action::RegisterCustodyVault(registration(1))),
        v.act(Action::AuthorizeSessionKey {
            key: bridge.addr(),
            scope: keel_actions::session_scope::CLIENT_OPS,
            expires_at: now + 3_600,
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    let operator_nonce = c.state.account_ref(&client).unwrap().nonce;

    // The bridge onboards the customer: attestation and deposit address.
    let (r, _) = c.block(&[
        bridge.act(Action::Attest {
            subject: customer,
            tier: 1,
            expires_at: T0 + 1_000_000,
        }),
        bridge.act(Action::RequestCustodyAddress {
            chain: ExtChain::Tron,
            custodian: client,
            owner: Some(customer),
        }),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert!(r[1].events.contains(&Event::CustodyAddressAssigned {
        custodian: client,
        owner: customer,
        chain: "TRON".into(),
        index: 1,
    }));
    // A deposit is credited to the customer, and the bridge moves it.
    fund_custody(&mut c.state, client, customer, &usdt(), 5_000_000);
    let (r, _) = c.block(&[bridge.act(transfer_custody(
        client,
        usdt(),
        2_000_000,
        Some(customer),
        "trade:9:escrow",
    ))]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, customer, &usdt()), 3_000_000);
    assert_eq!(custody::balance(&c.state, client, &usdt()), 2_000_000);
    // None of it used the operator's nonce: Harbor and the CLI sign with
    // the operator key in the meantime without colliding.
    assert_eq!(c.state.account_ref(&client).unwrap().nonce, operator_nonce);

    // The scope stops at operations: the session key cannot buy a plan,
    // pay out, or change fees.
    let (r, _) = c.block(&[
        bridge.act(Action::BuyPlan { tier: 1 }),
        bridge.act(Action::WithdrawCustody(Withdraw {
            asset: usdt(),
            to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
            amount: 1,
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    assert!(matches!(err(&r[1]), VmError::Unauthorized));
    // Payouts are a scope of their own: the operator gives it to the key
    // that should send coins out, and to no other.
    let payout = |amount: u128| Action::WithdrawCustodyFor {
        owner: customer,
        asset: usdt(),
        via: None,
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount,
        fee: None,
    };
    let (r, _) = c.block(&[bridge.act(payout(1_000_000))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    let mut cashier = Actor::new(91);
    let now = T0 + c.height + 1;
    let (r, _) = c.block(&[v.act(Action::AuthorizeSessionKey {
        key: cashier.addr(),
        scope: keel_actions::session_scope::CLIENT_PAYOUTS,
        expires_at: now + 3_600,
    })]);
    ok(&r[0]);
    // One action pays out of the keyless account: no transfer first.
    let (r, _) = c.block(&[cashier.act(payout(1_000_000))]);
    ok(&r[0]);
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::CustodyWithdrawalQueued { owner, amount: 1_000_000, .. } if *owner == customer
    )));
    assert_eq!(custody::balance(&c.state, customer, &usdt()), 2_000_000);
    assert_eq!(
        acct(&c.state, customer, &usdt(), "custody_escrow"),
        1_000_000
    );
    // The payout key does nothing else, a client pays out only of its
    // own accounts, and USDT has to name its network.
    let (r, _) = c.block(&[
        cashier.act(transfer_custody(client, usdt(), 1, Some(customer), "x")),
        alice.act(payout(1)),
        cashier.act(Action::WithdrawCustodyFor {
            owner: customer,
            asset: usds(),
            via: None,
            to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
            amount: 1,
            fee: None,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    assert!(matches!(err(&r[1]), VmError::Unauthorized));
    assert!(matches!(err(&r[2]), VmError::Invalid(_)));
    // The client's own withdrawal fee rides in the same action: paid to
    // its account as the payout is queued, or not at all.
    let with_fee = |amount: u128, fee: u128| Action::WithdrawCustodyFor {
        owner: customer,
        asset: usdt(),
        via: None,
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount,
        fee: Some(keel_actions::CustodyFee {
            to: client,
            amount: fee,
        }),
    };
    let fees_before = custody::balance(&c.state, client, &usdt());
    let (r, _) = c.block(&[cashier.act(with_fee(500_000, 20_000))]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, customer, &usdt()), 1_480_000);
    assert_eq!(
        custody::balance(&c.state, client, &usdt()),
        fees_before + 20_000
    );
    // Amount plus fee above the balance: nothing is charged, nothing queued.
    let (r, _) = c.block(&[cashier.act(with_fee(1_470_000, 20_000))]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    assert_eq!(custody::balance(&c.state, customer, &usdt()), 1_480_000);
    assert_eq!(
        custody::balance(&c.state, client, &usdt()),
        fees_before + 20_000
    );

    // Screening: the client has deposits to its users held. A deposit is
    // then in the reserve and owed, and not spendable, until released.
    let (r, _) = c.block(&[
        alice.act(Action::SetCustodyOptions {
            hold_deposits: true,
        }),
        v.act(Action::SetCustodyOptions {
            hold_deposits: true,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    ok(&r[1]);
    let spendable = custody::balance(&c.state, customer, &usds());
    let (r, _) = c.block(&[v.act(Action::ObserveCustodyDeposit {
        custodian: client,
        observation: obs(usdt(), 1, 7_000_000, 41),
    })]);
    ok(&r[0]);
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::CustodyDepositHeld { owner, amount: 7_000_000, .. } if *owner == customer
    )));
    assert!(!r[0]
        .events
        .iter()
        .any(|e| matches!(e, Event::CustodyDepositCredited { .. })));
    assert_eq!(acct(&c.state, customer, &usdt(), "custody_hold"), 7_000_000);
    assert_eq!(custody::balance(&c.state, customer, &usds()), spendable);
    let (r, _) =
        c.block(&[bridge.act(transfer_custody(client, usds(), 1, Some(customer), "held"))]);
    assert!(matches!(err(&r[0]), VmError::NotEnoughFunds));
    // Released in part to the user (who then holds USDT), and the flagged
    // part to the client's own account; only the custodian decides.
    let release = |amount: u128, to: Option<Address>| Action::ReleaseCustodyHold {
        owner: customer,
        asset: usdt(),
        amount,
        to,
    };
    let (r, _) = c.block(&[alice.act(release(1, None))]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    let (r, _) = c.block(&[
        bridge.act(release(5_000_000, None)),
        bridge.act(release(2_000_000, Some(client))),
        bridge.act(release(1, None)),
    ]);
    ok(&r[0]);
    ok(&r[1]);
    assert!(matches!(err(&r[2]), VmError::NotEnoughFunds));
    assert_eq!(acct(&c.state, customer, &usdt(), "custody_hold"), 0);
    assert_eq!(
        custody::balance(&c.state, customer, &usds()),
        spendable + 5_000_000
    );
    assert_eq!(custody::balance(&c.state, client, &usds()), 2_000_000);
    let (_, end) = c.block(&[]);
    assert!(!end
        .iter()
        .any(|e| matches!(e, Event::CustodyReserveBreached { .. })));
    // Nobody but the custodian asks for someone else's address.
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Tron,
        custodian: client,
        owner: Some(customer),
    })]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}
