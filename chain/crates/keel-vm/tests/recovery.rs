//! Self-service recovery: the owner of a deposit index (or its custodian)
//! asks, the observers report, the quorum closes the row; no one else has
//! a say, and a row nobody serves expires.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{
    Action, Chain as ExtChain, CustodyVaultRegistration, RecoveryObservation, RecoveryRequest,
};
use keel_types::Address;
use keel_vm::{modules::recovery::RecoveryStatus, Event, VmError};

fn request(chain: ExtChain, evm_chain_id: Option<u64>, token: Option<&str>, to: &str) -> Action {
    Action::RequestRecovery(RecoveryRequest {
        chain,
        evm_chain_id,
        token: token.map(str::to_string),
        to: to.into(),
        custodian: None,
        owner: None,
    })
}

fn observation(id: u64, amount: u128, success: bool, tx: u8) -> Action {
    Action::ObserveRecovery(RecoveryObservation {
        id,
        tx_hash: [tx; 32],
        external_tx: None,
        amount,
        external_height: 100,
        tip_height: 200,
        success,
    })
}

const EVM_TO: &str = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
const USDT_POLYGON: &str = "0xc2132D05D31c914a87C6611C10748AEb04B58e8F";

#[test]
fn owner_requests_observers_close_fee_charged_and_rows_expire() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    // Governance (the genesis param admin is the validator here through
    // SetParam? No: set the fee directly to keep the test on recovery).
    c.state.recovery.params.fee_keel = 1_000_000;
    c.state.recovery.params.expiry_blocks = 5;

    // No BSC address yet: nothing to recover from.
    let (r, _) = c.block(&[alice.act(request(
        ExtChain::Bsc,
        Some(137),
        Some(USDT_POLYGON),
        EVM_TO,
    ))]);
    assert!(matches!(r[0].error, Some(VmError::NotFound(_))));
    // Bitcoin has no recovery; a Solana id on a non-EVM chain is refused.
    let (r, _) = c.block(&[
        alice.act(request(
            ExtChain::Bitcoin,
            None,
            None,
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
        )),
        alice.act(request(
            ExtChain::Tron,
            Some(1),
            None,
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        )),
    ]);
    assert!(matches!(r[0].error, Some(VmError::Invalid(ref m)) if m.contains("Bitcoin")));
    assert!(matches!(r[1].error, Some(VmError::Invalid(ref m)) if m.contains("evm_chain_id")));

    // Alice gets a BSC (EVM family) address, then asks for USDT on Polygon
    // sent to it; bob, who has no address, cannot; a duplicate is refused.
    let before = acct(&c.state, alice.addr(), &keel(), "deposit");
    let (r, _) = c.block(&[alice.act(Action::RequestDepositAddress {
        chain: ExtChain::Bsc,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[
        alice.act(request(
            ExtChain::Bsc,
            Some(137),
            Some(USDT_POLYGON),
            EVM_TO,
        )),
        bob.act(request(
            ExtChain::Bsc,
            Some(137),
            Some(USDT_POLYGON),
            EVM_TO,
        )),
        alice.act(request(
            ExtChain::Bsc,
            Some(137),
            Some(USDT_POLYGON),
            EVM_TO,
        )),
        alice.act(request(ExtChain::Bsc, Some(137), Some("nope"), EVM_TO)),
        alice.act(request(
            ExtChain::Bsc,
            None,
            None,
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
        )),
    ]);
    ok(&r[0]);
    assert!(matches!(r[1].error, Some(VmError::NotFound(_))));
    assert!(matches!(r[2].error, Some(VmError::Invalid(ref m)) if m.contains("already open")));
    assert!(matches!(r[3].error, Some(VmError::Invalid(ref m)) if m.contains("token")));
    assert!(matches!(r[4].error, Some(VmError::Invalid(ref m)) if m.contains("destination")));
    assert_eq!(
        acct(&c.state, alice.addr(), &keel(), "deposit"),
        before - 1_000_000
    );
    let id = match &r[0].events[0] {
        Event::RecoveryRequested {
            id,
            owner,
            deposit_index,
            fee,
            evm_chain_id,
            ..
        } => {
            assert_eq!(*owner, alice.addr());
            assert_eq!(*deposit_index, 1);
            assert_eq!(*fee, 1_000_000);
            assert_eq!(*evm_chain_id, Some(137));
            *id
        }
        e => panic!("{e:?}"),
    };
    assert_eq!(c.state.recovery.rows[&id].status, RecoveryStatus::Open);
    // A native recovery on BSC itself is a distinct row.
    let (r, _) = c.block(&[alice.act(request(ExtChain::Bsc, None, None, EVM_TO))]);
    ok(&r[0]);
    let native_id = id + 1;

    // Only observers report; the quorum (one observer here) closes the row
    // with the median amount; a second vote is refused; a closed row takes
    // no more votes.
    let (r, _) = c.block(&[
        alice.act(observation(id, 5, true, 1)),
        v.act(observation(id, 25_000_000, true, 9)),
        v.act(observation(id, 1, true, 9)),
    ]);
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    ok(&r[1]);
    assert!(matches!(r[2].error, Some(VmError::Invalid(ref m)) if m.contains("not open")));
    let row = &c.state.recovery.rows[&id];
    assert_eq!(row.status, RecoveryStatus::Done);
    assert_eq!(row.amount, 25_000_000);
    assert_eq!(row.tx_hash, Some([9u8; 32]));
    assert!(
        matches!(r[1].events[0], Event::RecoveryDone { id: i, amount: 25_000_000, .. } if i == id)
    );
    // Once closed, the same recovery may be asked again (coins can land twice).
    let (r, _) = c.block(&[alice.act(request(
        ExtChain::Bsc,
        Some(137),
        Some(USDT_POLYGON),
        EVM_TO,
    ))]);
    ok(&r[0]);
    let again = native_id + 1;
    // A failed transaction closes the row as Failed.
    let (r, _) = c.block(&[v.act(observation(again, 0, false, 3))]);
    ok(&r[0]);
    assert_eq!(c.state.recovery.rows[&again].status, RecoveryStatus::Failed);
    assert!(matches!(r[0].events[0], Event::RecoveryFailed { id: i, .. } if i == again));

    // The native row nobody served expires after `expiry_blocks`, and
    // closed rows are forgotten one period later.
    let created = c.state.recovery.rows[&native_id].created_height;
    let events = c.advance_to(created + 5);
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::RecoveryExpired { id: i } if *i == native_id)));
    assert_eq!(
        c.state.recovery.rows[&native_id].status,
        RecoveryStatus::Expired
    );
    c.advance_to(created + 12);
    assert!(!c.state.recovery.rows.contains_key(&id));
    assert!(!c.state.recovery.rows.contains_key(&native_id));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

/// A custody vault: the custodian (the attester) asks for its user's
/// index; the user alone, or a stranger, cannot.
#[test]
fn custodian_asks_for_its_users_index() {
    let (state, mut alice, mut bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    // The validator is the genesis attester: it registers a Solana custody
    // vault, attests alice, alice asks for an address in it.
    let (r, _) = c.block(&[
        v.act(Action::RegisterCustodyVault(CustodyVaultRegistration {
            chain: ExtChain::Solana,
            epoch: 1,
            public_key: vec![9u8; 32],
            chain_code: Some([7u8; 32]),
            signer_url: "https://signer.example.com".into(),
        })),
    ]);
    ok(&r[0]);
    let exp = T0 + 100 * 86_400;
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: exp,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Solana,
        custodian: v.addr(),
    })]);
    ok(&r[0]);
    let ask = |custodian: Option<Address>, owner: Option<Address>| {
        Action::RequestRecovery(RecoveryRequest {
            chain: ExtChain::Solana,
            evm_chain_id: None,
            token: Some("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU".into()),
            to: "9Yt1UNy1t2MULrRbN6PG9mAD6prjWDfDpnTBGP1F1Sqz".into(),
            custodian,
            owner,
        })
    };
    let (r, _) = c.block(&[
        // alice's own network index does not exist (custody only).
        alice.act(ask(None, None)),
        // bob is nobody to that vault.
        bob.act(ask(Some(v.addr()), Some(alice.addr()))),
        // the custodian, without naming whose index.
        v.act(ask(Some(v.addr()), None)),
        // the custodian, for alice.
        v.act(ask(Some(v.addr()), Some(alice.addr()))),
    ]);
    assert!(matches!(r[0].error, Some(VmError::NotFound(_))));
    assert_eq!(r[1].error, Some(VmError::Unauthorized));
    assert!(matches!(r[2].error, Some(VmError::Invalid(ref m)) if m.contains("owner")));
    ok(&r[3]);
    let row = c.state.recovery.rows.values().next().unwrap();
    assert_eq!(row.owner, alice.addr());
    assert_eq!(row.custodian, Some(v.addr()));
    assert_eq!(row.deposit_index, 1);
    assert_eq!(row.chain, ExtChain::Solana);
}
