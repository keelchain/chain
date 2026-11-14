//! Custody fresh-pool rotation: a fresh key becomes the active generation,
//! the old one drains (still credited, still paying) until its grace is
//! over and the observers report it empty, sweeps are attested by quorum
//! and their fee charged to the custodian, scheduled rotations open in
//! `end_block`, and registration can no longer overwrite a key.
#![allow(clippy::unwrap_used)]

mod common;

use common::*;
use keel_actions::{
    Action, Chain as ExtChain, CustodyRotation, CustodyVaultRegistration, DepositObservation,
    Proof, SweepObservation, Withdraw,
};
use keel_ledger::{Record, TxType};
use keel_types::{Address, Asset};
use keel_vm::{
    modules::{
        custody,
        pools::{self, GenStatus},
        vaults::OutboundStatus,
    },
    Event, State, VmError,
};

fn usdt() -> Asset {
    Asset::new("TRON.USDT")
}
fn trx() -> Asset {
    Asset::new("TRON.TRX")
}

fn registration() -> CustodyVaultRegistration {
    CustodyVaultRegistration {
        chain: ExtChain::Tron,
        epoch: 1,
        public_key: vec![2u8; 33],
        chain_code: Some([7u8; 32]),
        signer_url: "https://signer-a.example.com".into(),
    }
}

fn rotation(key: u8, url: &str, sweep: bool, activate_at: Option<u64>) -> Action {
    Action::RotateCustodyVault(CustodyRotation {
        chain: ExtChain::Tron,
        public_key: vec![key; 33],
        chain_code: [key; 32],
        signer_url: url.into(),
        sweep,
        activate_at,
    })
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

fn statuses(c: &Chain, client: Address) -> Vec<(u64, GenStatus)> {
    pools::generations(&c.state, ExtChain::Tron, client)
        .iter()
        .map(|g| (g.gen, g.status))
        .collect()
}

#[test]
fn rotation_opens_a_fresh_generation_and_the_old_one_drains() {
    let (state, mut alice, _bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    c.state.pools.params.grace_blocks = 5;
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(registration()))]);
    ok(&r[0]);
    // Before any rotation: generation 0 is the registration itself.
    assert_eq!(statuses(&c, client), vec![(0, GenStatus::Active)]);
    // A registered vault never changes key by re-registering.
    let (r, _) = c.block(&[
        v.act(Action::RegisterCustodyVault(CustodyVaultRegistration {
            epoch: 2,
            ..registration()
        })),
    ]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("RotateCustodyVault")));

    // alice is in custody with 5 USDT at index 1.
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    let (r, _) = c.block(&[alice.act(Action::RequestCustodyAddress {
        chain: ExtChain::Tron,
        custodian: client,
    })]);
    ok(&r[0]);
    fund_custody(&mut c.state, client, alice.addr(), &usdt(), 5_000_000);

    // Refusals: stranger, same key, same signer URL, bad URL, past height.
    let (r, _) = c.block(&[
        alice.act(rotation(3, "https://signer-b.example.com", false, None)),
        v.act(rotation(2, "https://signer-b.example.com", false, None)),
        v.act(rotation(3, "https://signer-a.example.com", false, None)),
        v.act(rotation(3, "ftp://nope", false, None)),
        v.act(rotation(3, "https://signer-b.example.com", false, Some(1))),
    ]);
    assert!(matches!(err(&r[0]), VmError::NotFound(_)));
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    assert!(matches!(err(&r[2]), VmError::Invalid(_)));
    assert!(matches!(err(&r[3]), VmError::Invalid(_)));
    assert!(matches!(err(&r[4]), VmError::Invalid(_)));

    // The rotation: generation 1 is active, 0 drains for the grace.
    let (r, _) = c.block(&[v.act(rotation(3, "https://signer-b.example.com", false, None))]);
    ok(&r[0]);
    let h = c.height;
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::PoolRotated { gen: 1, previous: 0, drain_until, .. } if *drain_until == h + 5
    )));
    assert_eq!(
        statuses(&c, client),
        vec![(0, GenStatus::Draining), (1, GenStatus::Active)]
    );
    let active = pools::active(&c.state, ExtChain::Tron, client).unwrap();
    assert_eq!(active.public_key, vec![3u8; 33]);
    assert_eq!(active.signer_url, "https://signer-b.example.com");
    // The registration record follows the fresh key, with the epoch bumped.
    let vault = c.state.custody.vault(ExtChain::Tron, client).unwrap();
    assert_eq!(vault.public_key, vec![3u8; 33]);
    assert_eq!(vault.epoch, 2);
    assert_eq!(vault.signer_url, "https://signer-b.example.com");
    // The old generation keeps its own key and signer.
    let old = &pools::generations(&c.state, ExtChain::Tron, client)[0];
    assert_eq!(old.public_key, vec![2u8; 33]);
    assert_eq!(old.signer_url, "https://signer-a.example.com");
    assert_eq!(pools::live(&c.state, ExtChain::Tron, client).len(), 2);

    // A deposit to the old generation's index still credits alice: the
    // index means the same account under every key.
    let (r, _) = c.block(&[v.act(Action::ObserveCustodyDeposit {
        custodian: client,
        observation: obs(usdt(), 1, 1_000_000, 1),
    })]);
    ok(&r[0]);
    assert_eq!(custody::balance(&c.state, alice.addr(), &usdt()), 6_000_000);

    // A withdrawal batch is pointed at the draining generation (no sweep),
    // so the old pool drains through ordinary payouts.
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
    let batch = c.state.vaults.outbounds[&id].batch_id.unwrap();
    assert_eq!(
        c.state.vaults.outbounds[&id].status,
        OutboundStatus::Batched
    );
    assert_eq!(c.state.pools.batch_gen.get(&batch), Some(&0));

    // Retirement: not before the grace; then one observer report reaches
    // the devnet threshold of one.
    let (r, _) = c.block(&[v.act(Action::RetireCustodyGeneration {
        chain: ExtChain::Tron,
        custodian: client,
        gen: 0,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("drains until")));
    c.advance_to(h + 5);
    let (r, _) = c.block(&[
        alice.act(Action::RetireCustodyGeneration {
            chain: ExtChain::Tron,
            custodian: client,
            gen: 0,
        }),
        v.act(Action::RetireCustodyGeneration {
            chain: ExtChain::Tron,
            custodian: client,
            gen: 0,
        }),
    ]);
    assert!(matches!(err(&r[0]), VmError::Unauthorized));
    ok(&r[1]);
    assert!(r[1]
        .events
        .iter()
        .any(|e| matches!(e, Event::PoolGenerationRetired { gen: 0, .. })));
    assert_eq!(
        statuses(&c, client),
        vec![(0, GenStatus::Retired), (1, GenStatus::Active)]
    );
    assert_eq!(pools::live(&c.state, ExtChain::Tron, client).len(), 1);
    // Later batches go to the active generation.
    let (r, _) = c.block(&[alice.act(Action::WithdrawCustody(Withdraw {
        asset: usdt(),
        to: "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE".into(),
        amount: 1_000_000,
    }))]);
    ok(&r[0]);
    let batch2 = c.state.vaults.outbounds[&(id + 1)].batch_id.unwrap();
    assert_eq!(c.state.pools.batch_gen.get(&batch2), Some(&1));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn sweep_is_attested_by_quorum_and_charged_to_the_custodian() {
    let (state, mut alice, _bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(registration()))]);
    ok(&r[0]);
    let (r, _) = c.block(&[v.act(Action::Attest {
        subject: alice.addr(),
        tier: 1,
        expires_at: T0 + 1_000_000,
    })]);
    ok(&r[0]);
    fund_custody(&mut c.state, client, alice.addr(), &usdt(), 5_000_000);
    // The client's gas tank in its own vault: 3 TRX at index 0.
    fund_custody(&mut c.state, client, client, &trx(), 3_000_000);
    let (r, _) = c.block(&[v.act(rotation(3, "https://signer-b.example.com", true, None))]);
    ok(&r[0]);
    assert!(pools::generations(&c.state, ExtChain::Tron, client)[0].sweep);
    // With sweep on, batches are paid from the active generation.
    assert_eq!(pools::batch_generation(&c.state, ExtChain::Tron, client), 1);

    // Only observers attest sweeps; only draining generations are swept.
    let sweep = |from_gen: u64, tx: u8, amount: u128, fee: u128| {
        Action::ObserveCustodySweep(SweepObservation {
            chain: ExtChain::Tron,
            custodian: client,
            from_gen,
            tx_hash: [tx; 32],
            asset: usdt(),
            amount,
            fee,
        })
    };
    let (r, _) = c.block(&[
        alice.act(sweep(0, 9, 5_000_000, 1_000_000)),
        v.act(sweep(1, 9, 5_000_000, 1_000_000)),
        v.act(sweep(0, 9, 5_000_000, 1_000_000)),
        v.act(sweep(0, 9, 5_000_000, 1_000_000)),
    ]);
    assert!(!r[0].ok);
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    ok(&r[2]);
    assert!(r[2].events.iter().any(|e| matches!(
        e,
        Event::PoolSwept {
            from_gen: 0,
            amount: 5_000_000,
            fee: 1_000_000,
            ..
        }
    )));
    assert!(matches!(err(&r[3]), VmError::Invalid(m) if m.contains("already")));
    // The fee came out of the client's gas; USDT balances are untouched
    // (coins moved between the client's own keys).
    assert_eq!(custody::balance(&c.state, client, &trx()), 2_000_000);
    assert_eq!(custody::balance(&c.state, alice.addr(), &usdt()), 5_000_000);
    assert_eq!(
        pools::generations(&c.state, ExtChain::Tron, client)[0]
            .swept
            .get(&usdt()),
        Some(&5_000_000)
    );
    assert!(c.state.custody.halted.is_empty());
    assert!(c.state.ledger.audit().mismatches.is_empty());
}

#[test]
fn scheduled_rotation_opens_in_end_block_and_can_be_cancelled() {
    let (state, _alice, _bob, mut v) = setup(0);
    let mut c = Chain::new(state);
    let client = v.addr();
    let (r, _) = c.block(&[v.act(Action::RegisterCustodyVault(registration()))]);
    ok(&r[0]);
    let at = c.height + 10;
    let (r, _) = c.block(&[v.act(rotation(3, "https://signer-b.example.com", false, Some(at)))]);
    ok(&r[0]);
    assert!(r[0].events.iter().any(|e| matches!(
        e,
        Event::PoolRotationScheduled { gen: 1, activate_at, .. } if *activate_at == at
    )));
    assert!(pools::rotation(&c.state, ExtChain::Tron, client).is_some());
    // Still the old key until then; a second rotation is refused meanwhile.
    assert_eq!(statuses(&c, client), vec![(0, GenStatus::Active)]);
    let (r, _) = c.block(&[v.act(rotation(4, "https://signer-c.example.com", false, None))]);
    assert!(matches!(err(&r[0]), VmError::Invalid(m) if m.contains("pending")));
    // Cancel, then schedule again and let it open.
    let (r, _) = c.block(&[v.act(Action::CancelCustodyRotation {
        chain: ExtChain::Tron,
    })]);
    ok(&r[0]);
    assert!(pools::rotation(&c.state, ExtChain::Tron, client).is_none());
    let (r, _) = c.block(&[v.act(Action::CancelCustodyRotation {
        chain: ExtChain::Tron,
    })]);
    assert!(matches!(err(&r[0]), VmError::Invalid(_)));
    let at = c.height + 3;
    let (r, _) = c.block(&[v.act(rotation(3, "https://signer-b.example.com", false, Some(at)))]);
    ok(&r[0]);
    let events = c.advance_to(at);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::PoolRotated {
            gen: 1,
            previous: 0,
            ..
        }
    )));
    assert_eq!(
        statuses(&c, client),
        vec![(0, GenStatus::Draining), (1, GenStatus::Active)]
    );
    assert_eq!(
        c.state
            .custody
            .vault(ExtChain::Tron, client)
            .unwrap()
            .public_key,
        vec![3u8; 33]
    );
    // Params through the admin.
    c.state.gov.param_admin = Some(client);
    let (r, _) = c.block(&[
        v.act(Action::SetParam {
            key: "pools.grace_blocks".into(),
            value: 50,
        }),
        v.act(Action::SetParam {
            key: "pools.grace_blocks".into(),
            value: 0,
        }),
    ]);
    ok(&r[0]);
    assert!(matches!(err(&r[1]), VmError::Invalid(_)));
    assert_eq!(pools::get_param(&c.state, "pools.grace_blocks"), Some(50));
    assert!(c.state.ledger.audit().mismatches.is_empty());
}
