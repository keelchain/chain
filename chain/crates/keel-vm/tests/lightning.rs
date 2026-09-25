//! Lightning pools (2026-09-10): fund from the vault, credit invoice
//! deposits, pay invoices, time out, sweep back. Every limit is a param.
#![allow(clippy::unwrap_used)]

use bitcoin::{
    block::{Header, Version},
    blockdata::constants::genesis_block,
    consensus::serialize,
    hashes::{sha256d, Hash},
    merkle_tree::PartialMerkleTree,
    secp256k1::{Secp256k1, SecretKey},
    Network, TxMerkleNode, Txid,
};
use keel_actions::{
    Action, Chain, DepositObservation, LightningDepositObservation, OutboundObservation, Proof,
    SignedAction, VaultRegistration, Withdraw, CHAIN_ID_DEVNET,
};
use keel_crypto::Keypair;
use keel_ledger::AccountKey;
use keel_types::{Address, Asset};
use keel_vm::{
    apply_block,
    genesis::GenesisValidator,
    modules::{tokens, vaults},
    BlockContext, Event, Genesis, State, VmError,
};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use sha2::Digest;

struct Actor {
    key: Keypair,
    nonce: u64,
}
impl Actor {
    fn new(seed: u64) -> Self {
        Self {
            key: Keypair::from_seed(seed),
            nonce: 0,
        }
    }
    fn addr(&self) -> Address {
        self.key.address()
    }
    fn act(&mut self, a: Action) -> SignedAction {
        let s = SignedAction::sign(&self.key, self.nonce, CHAIN_ID_DEVNET, a);
        self.nonce += 1;
        s
    }
}

fn setup() -> (State, Actor, Vec<Actor>) {
    let alice = Actor::new(1);
    let observers: Vec<Actor> = (100..103).map(Actor::new).collect();
    let validators = observers
        .iter()
        .map(|o| GenesisValidator {
            address: o.addr(),
            consensus_key: o.addr().0,
            bond: 0,
        })
        .collect();
    let g = Genesis::devnet(CHAIN_ID_DEVNET, &[alice.addr()], validators);
    let mut state = g.build();
    let signers: Vec<Address> = observers.iter().map(|o| o.addr()).collect();
    let mut obs0 = Actor::new(100);
    ok(
        &mut state,
        1,
        &[obs0.act(Action::RegisterVault(VaultRegistration {
            chain: Chain::Bitcoin,
            epoch: 1,
            public_key: vec![2u8; 33],
            chain_code: Some([1u8; 32]),
            signers,
            threshold: 2,
        }))],
    );
    let mut observers = observers;
    observers[0].nonce = obs0.nonce;
    (state, alice, observers)
}

fn ctx(height: u64) -> BlockContext {
    BlockContext {
        height,
        timestamp: 1_700_000_000_000 + height * 250,
        proposer: None,
    }
}
fn btc() -> Asset {
    Asset::new("BTC.BTC")
}
fn sys(t: &str) -> AccountKey {
    AccountKey::new(Address::SYSTEM, btc(), t).unwrap()
}
fn ok(state: &mut State, height: u64, actions: &[SignedAction]) -> Vec<keel_vm::Receipt> {
    let (r, _) = apply_block(state, &ctx(height), actions);
    for x in &r {
        assert!(x.ok, "action {} failed: {:?}", x.index, x.error);
    }
    r
}
fn err(state: &mut State, height: u64, action: SignedAction) -> VmError {
    let (r, _) = apply_block(state, &ctx(height), &[action]);
    r[0].error.clone().expect("expected failure")
}

/// A Lightning node key and the invoices it signs.
struct LnNode {
    sk: SecretKey,
    secp: Secp256k1<bitcoin::secp256k1::All>,
}
impl LnNode {
    fn new(byte: u8) -> Self {
        Self {
            sk: SecretKey::from_slice(&[byte; 32]).unwrap(),
            secp: Secp256k1::new(),
        }
    }
    fn id(&self) -> Vec<u8> {
        self.sk.public_key(&self.secp).serialize().to_vec()
    }
    fn invoice(
        &self,
        description: &str,
        msat: u64,
        preimage: [u8; 32],
        now_secs: u64,
        expiry: u64,
    ) -> String {
        let hash: [u8; 32] = sha2::Sha256::digest(preimage).into();
        InvoiceBuilder::new(Currency::Regtest)
            .description(description.into())
            .payment_hash(bitcoin::hashes::sha256::Hash::from_slice(&hash).unwrap())
            .payment_secret(PaymentSecret([9u8; 32]))
            .amount_milli_satoshis(msat)
            .duration_since_epoch(std::time::Duration::from_secs(now_secs))
            .min_final_cltv_expiry_delta(18)
            .expiry_time(std::time::Duration::from_secs(expiry))
            .build_signed(|h| self.secp.sign_ecdsa_recoverable(h, &self.sk))
            .unwrap()
            .to_string()
    }
}

fn now_secs(height: u64) -> u64 {
    ctx(height).timestamp / 1000
}

/// Fund observer 0's pool with `amount` sats from the vault (outbound + quorum).
fn fund(state: &mut State, obs: &mut [Actor], amount: u128, height: u64) -> u64 {
    let r = ok(
        state,
        height,
        &[obs[0].act(Action::FundLightningPool {
            amount,
            to: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".into(),
        })],
    );
    let id = match r[0]
        .events
        .iter()
        .find(|e| matches!(e, Event::LightningPoolFunded { .. }))
    {
        Some(Event::LightningPoolFunded { outbound_id, .. }) => *outbound_id,
        _ => panic!("no funding event"),
    };
    let interval = state.params.outbound_batch_interval_blocks;
    let batch_height = height.div_ceil(interval) * interval;
    let (_, end) = apply_block(state, &ctx(batch_height), &[]);
    assert!(
        end.iter()
            .any(|e| matches!(e, Event::OutboundBatched { outbound_id, .. } if *outbound_id == id)),
        "{end:?}"
    );
    let o = |fee| OutboundObservation {
        outbound_id: id,
        tx_hash: [0xbb; 32],
        external_height: 10,
        tip_height: 13,
        fee_paid: fee,
        success: true,
    };
    ok(
        state,
        batch_height + 1,
        &[
            obs[0].act(Action::ObserveOutbound(o(1_000))),
            obs[1].act(Action::ObserveOutbound(o(1_000))),
        ],
    );
    id
}

#[test]
fn pool_funding_moves_reserves_from_vault_to_pool_under_the_cap() {
    let (mut state, _alice, mut obs) = setup();
    let node = LnNode::new(0x11);
    assert!(matches!(
        err(
            &mut state,
            2,
            obs[0].act(Action::RegisterLightningNode {
                node_id: vec![1, 2, 3]
            })
        ),
        VmError::Invalid(_)
    ));
    assert!(
        matches!(
            err(
                &mut state,
                2,
                obs[0].act(Action::FundLightningPool {
                    amount: 1,
                    to: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".into()
                })
            ),
            VmError::Invalid(_)
        ),
        "no node yet"
    );
    ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::RegisterLightningNode { node_id: node.id() }),
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 5,
            }),
        ],
    );
    let vault_before = state.ledger.balance(&sys("vault_asset"));
    state.params.lightning_pool_cap_sats = 5_000_000;
    assert!(
        matches!(err(&mut state, 3, obs[0].act(Action::FundLightningPool { amount: 5_000_001, to: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".into() })), VmError::Invalid(ref m) if m.contains("cap"))
    );
    fund(&mut state, &mut obs, 5_000_000, 3);
    assert_eq!(state.ledger.balance(&sys("lightning_pool")), 5_000_000);
    // Amount plus the mining fee left the vault; the fee is an expense.
    assert_eq!(
        state.ledger.balance(&sys("vault_asset")),
        vault_before - 5_000_000 - 1_000
    );
    assert_eq!(state.ledger.balance(&sys("sweep_gas")), 1_000);
    let pool = &state.lightning.pools[&obs[0].addr()];
    assert_eq!((pool.balance, pool.pending_out), (5_000_000, 0));
    // Reserves still cover liabilities: pool + vault count as reserves.
    assert!(state.ledger.system_reserves(&btc()) >= state.ledger.user_liabilities(&btc()) as i128);
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn invoice_deposits_credit_the_owner_and_are_verified() {
    let (mut state, alice, mut obs) = setup();
    let node = LnNode::new(0x11);
    let other = LnNode::new(0x22);
    ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::RegisterLightningNode { node_id: node.id() }),
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 5,
            }),
        ],
    );
    fund(&mut state, &mut obs, 1_000_000, 3);
    let desc = keel_ln::deposit_description(&alice.addr().to_hex());
    let preimage = [7u8; 32];
    let inv = node.invoice(&desc, 2_500_000, preimage, now_secs(30), 3_600);
    let before = tokens::balance(&state, alice.addr(), &btc());
    let observe = |invoice: String, preimage: [u8; 32], msat: u64| {
        Action::ObserveLightningDeposit(LightningDepositObservation {
            invoice,
            preimage,
            amount_msat: msat,
        })
    };
    // Wrong preimage, foreign payee, unbound description, non-observer: refused.
    assert!(
        matches!(err(&mut state, 31, obs[0].act(observe(inv.clone(), [8u8; 32], 2_500_000))), VmError::Invalid(ref m) if m.contains("preimage"))
    );
    let foreign = other.invoice(&desc, 2_500_000, preimage, now_secs(30), 3_600);
    assert!(matches!(
        err(
            &mut state,
            31,
            obs[0].act(observe(foreign, preimage, 2_500_000))
        ),
        VmError::Unauthorized
    ));
    let unbound = node.invoice("coffee", 2_500_000, preimage, now_secs(30), 3_600);
    assert!(
        matches!(err(&mut state, 31, obs[0].act(observe(unbound, preimage, 2_500_000))), VmError::Invalid(ref m) if m.contains("bound"))
    );
    let mut mallory = Actor::new(7);
    assert!(matches!(
        err(
            &mut state,
            31,
            mallory.act(observe(inv.clone(), preimage, 2_500_000))
        ),
        VmError::Unauthorized
    ));
    // Under the invoice amount: refused; exact: credited once.
    assert!(matches!(
        err(
            &mut state,
            31,
            obs[0].act(observe(inv.clone(), preimage, 2_499_999))
        ),
        VmError::Invalid(_)
    ));
    let r = ok(
        &mut state,
        32,
        &[obs[0].act(observe(inv.clone(), preimage, 2_500_000))],
    );
    assert!(r[0].events.iter().any(|e| matches!(e, Event::LightningDepositCredited { owner, amount, .. } if *owner == alice.addr() && *amount == 2_500)));
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        before + 2_500
    );
    assert_eq!(state.lightning.pools[&obs[0].addr()].balance, 1_002_500);
    assert_eq!(state.ledger.balance(&sys("lightning_pool")), 1_002_500);
    assert!(
        matches!(err(&mut state, 33, obs[0].act(observe(inv, preimage, 2_500_000))), VmError::Invalid(ref m) if m.contains("already"))
    );
    // Params gate everything: max deposit, pool cap, daily cap, kill switch.
    state.params.lightning_max_deposit_sats = 1_000;
    let small = node.invoice(&desc, 1_001_000, [1u8; 32], now_secs(34), 3_600);
    assert!(
        matches!(err(&mut state, 34, obs[0].act(observe(small, [1u8; 32], 1_001_000))), VmError::Invalid(ref m) if m.contains("max_deposit"))
    );
    state.params.lightning_max_deposit_sats = 10_000_000;
    state.params.lightning_daily_cap_sats = 3_000;
    let daily = node.invoice(&desc, 1_000_000, [2u8; 32], now_secs(35), 3_600);
    assert!(
        matches!(err(&mut state, 35, obs[0].act(observe(daily.clone(), [2u8; 32], 1_000_000))), VmError::Invalid(ref m) if m.contains("daily"))
    );
    state.params.lightning_daily_cap_sats = 0;
    state.params.lightning_enabled = 0;
    assert!(matches!(
        err(
            &mut state,
            36,
            obs[0].act(observe(daily.clone(), [2u8; 32], 1_000_000))
        ),
        VmError::Paused(_)
    ));
    state.params.lightning_enabled = 1;
    ok(
        &mut state,
        37,
        &[obs[0].act(observe(daily, [2u8; 32], 1_000_000))],
    );
    assert!(state.ledger.audit().mismatches.is_empty());
}

#[test]
fn payouts_are_assigned_settled_or_refunded() {
    let (mut state, mut alice, mut obs) = setup();
    let node = LnNode::new(0x11);
    let payee = LnNode::new(0x33);
    ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::RegisterLightningNode { node_id: node.id() }),
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 5,
            }),
        ],
    );
    fund(&mut state, &mut obs, 1_000_000, 3);
    let preimage = [5u8; 32];
    let inv = payee.invoice("alice's coffee", 1_000_000, preimage, now_secs(40), 3_600);
    let before = tokens::balance(&state, alice.addr(), &btc());
    // Amount must match the invoice; fee allowance = max(0.5%, 10 sats) = 10.
    assert!(
        matches!(err(&mut state, 40, alice.act(Action::Withdraw(Withdraw { asset: btc(), to: inv.clone(), amount: 999 }))), VmError::Invalid(ref m) if m.contains("equal the invoice"))
    );
    let r = ok(
        &mut state,
        40,
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to: inv.clone(),
            amount: 1_000,
        }))],
    );
    let id = match r[0]
        .events
        .iter()
        .find(|e| matches!(e, Event::LightningPayoutAssigned { .. }))
    {
        Some(Event::LightningPayoutAssigned {
            outbound_id,
            observer,
        }) => {
            assert_eq!(*observer, obs[0].addr());
            *outbound_id
        }
        _ => panic!("not assigned"),
    };
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        before - 1_010
    );
    assert_eq!(state.lightning.pools[&obs[0].addr()].pending_out, 1_010);
    // Only the assigned observer may report; success needs the right preimage.
    assert!(matches!(
        err(
            &mut state,
            41,
            obs[1].act(Action::ObserveLightningPayout {
                outbound_id: id,
                preimage: Some(preimage),
                fee_paid_msat: 3_000,
                success: true
            })
        ),
        VmError::Unauthorized
    ));
    assert!(matches!(
        err(
            &mut state,
            41,
            obs[0].act(Action::ObserveLightningPayout {
                outbound_id: id,
                preimage: Some([6u8; 32]),
                fee_paid_msat: 3_000,
                success: true
            })
        ),
        VmError::Invalid(_)
    ));
    let r = ok(
        &mut state,
        42,
        &[obs[0].act(Action::ObserveLightningPayout {
            outbound_id: id,
            preimage: Some(preimage),
            fee_paid_msat: 3_000,
            success: true,
        })],
    );
    assert!(r[0].events.contains(&Event::LightningPayoutSettled {
        outbound_id: id,
        observer: obs[0].addr(),
        fee_paid: 3
    }));
    // 1,000 + 3 sats left; 7 of the 10-sat allowance came back.
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        before - 1_003
    );
    let pool = &state.lightning.pools[&obs[0].addr()];
    assert_eq!((pool.balance, pool.pending_out), (1_000_000 - 1_003, 0));
    assert_eq!(
        state.ledger.balance(&sys("lightning_pool")),
        1_000_000 - 1_003
    );
    assert_eq!(
        state.vaults.outbounds[&id].status,
        vaults::OutboundStatus::Confirmed
    );

    // A second payout that the observer never reports is refunded at the deadline.
    state.params.lightning_payout_timeout_blocks = 5;
    let inv2 = payee.invoice("later", 2_000_000, [4u8; 32], now_secs(50), 3_600);
    let r = ok(
        &mut state,
        50,
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to: inv2,
            amount: 2_000,
        }))],
    );
    let id2 = match r[0]
        .events
        .iter()
        .find(|e| matches!(e, Event::LightningPayoutAssigned { .. }))
    {
        Some(Event::LightningPayoutAssigned { outbound_id, .. }) => *outbound_id,
        _ => panic!("not assigned"),
    };
    let mid = tokens::balance(&state, alice.addr(), &btc());
    let (_, end) = apply_block(&mut state, &ctx(54), &[]);
    assert!(end.is_empty());
    let (_, end) = apply_block(&mut state, &ctx(55), &[]);
    assert!(end.contains(&Event::LightningPayoutFailed {
        outbound_id: id2,
        refunded: 2_010
    }));
    assert_eq!(tokens::balance(&state, alice.addr(), &btc()), mid + 2_010);
    assert_eq!(state.lightning.pools[&obs[0].addr()].pending_out, 0);
    // No capacity for more than the pool holds.
    let big = payee.invoice("too big", 5_000_000_000, [3u8; 32], now_secs(56), 3_600);
    assert!(
        matches!(err(&mut state, 56, alice.act(Action::Withdraw(Withdraw { asset: btc(), to: big, amount: 5_000_000 }))), VmError::Invalid(ref m) if m.contains("capacity"))
    );
    assert!(state.ledger.audit().mismatches.is_empty());
}

// ---- sweep: pool BTC returns to the vault's index-0 address via a normal SPV deposit ----

fn txid(n: u8) -> Txid {
    Txid::from_byte_array(sha256d::Hash::hash(&[n]).to_byte_array())
}
fn mine(prev: &Header, merkle_root: TxMerkleNode, time: u32) -> Header {
    let mut h = Header {
        version: Version::TWO,
        prev_blockhash: prev.block_hash(),
        merkle_root,
        time,
        bits: prev.bits,
        nonce: 0,
    };
    while h.validate_pow(h.target()).is_err() {
        h.nonce += 1;
    }
    h
}
fn btc_chain(n: usize, txids: &[Txid]) -> (Vec<Vec<u8>>, Vec<u8>) {
    let genesis = genesis_block(Network::Regtest).header;
    let matches: Vec<bool> = (0..txids.len()).map(|i| i == 1).collect();
    let pmt = PartialMerkleTree::from_txids(txids, &matches);
    let mut m = Vec::new();
    let mut idx = Vec::new();
    let root = pmt.extract_matches(&mut m, &mut idx).unwrap();
    let mut headers = vec![genesis];
    for i in 0..n {
        let prev = *headers.last().unwrap();
        let mr = if i == 0 {
            root
        } else {
            TxMerkleNode::from_byte_array(sha256d::Hash::hash(&[i as u8]).to_byte_array())
        };
        headers.push(mine(&prev, mr, prev.time + 600));
    }
    (
        headers[1..].iter().map(serialize).collect(),
        serialize(&pmt),
    )
}

#[test]
fn sweeps_return_pool_balance_to_the_vault() {
    let (mut state, _alice, mut obs) = setup();
    let node = LnNode::new(0x11);
    ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::RegisterLightningNode { node_id: node.id() }),
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 5,
            }),
        ],
    );
    fund(&mut state, &mut obs, 1_000_000, 3);
    let txids = vec![txid(1), txid(2), txid(3)];
    let (headers, proof) = btc_chain(4, &txids);
    let sweep_tx = txid(2).to_byte_array();
    let observation = || DepositObservation {
        chain: Chain::Bitcoin,
        asset: btc(),
        tx_hash: sweep_tx,
        index: 1,
        deposit_index: 0,
        amount: 400_000,
        external_height: 100,
        tip_height: 103,
        proof: Proof::Bitcoin {
            headers: headers.clone(),
            merkle_proof: proof.clone(),
            tx_index: 1,
        },
    };
    // Unannounced: refused before any vote is recorded.
    let (r, _) = apply_block(
        &mut state,
        &ctx(30),
        &[
            obs[0].act(Action::ObserveDeposit(observation())),
            obs[1].act(Action::ObserveDeposit(observation())),
        ],
    );
    assert!(r
        .iter()
        .all(|x| matches!(x.error, Some(VmError::Invalid(ref m)) if m.contains("not announced"))));
    assert!(state.vaults.pending.is_empty());
    assert!(
        matches!(err(&mut state, 31, obs[0].act(Action::AnnounceLightningSweep { tx_hash: sweep_tx, amount: 2_000_000 })), VmError::Invalid(ref m) if m.contains("exceeds"))
    );
    ok(
        &mut state,
        31,
        &[obs[0].act(Action::AnnounceLightningSweep {
            tx_hash: sweep_tx,
            amount: 400_000,
        })],
    );
    let vault_before = state.ledger.balance(&sys("vault_asset"));
    let r = ok(
        &mut state,
        32,
        &[
            obs[0].act(Action::ObserveDeposit(observation())),
            obs[1].act(Action::ObserveDeposit(observation())),
        ],
    );
    assert!(r.iter().flat_map(|x| x.events.iter()).any(|e| matches!(
        e,
        Event::LightningPoolSwept {
            amount: 400_000,
            ..
        }
    )));
    assert_eq!(
        state.ledger.balance(&sys("vault_asset")),
        vault_before + 400_000
    );
    assert_eq!(state.ledger.balance(&sys("lightning_pool")), 600_000);
    assert_eq!(state.lightning.pools[&obs[0].addr()].balance, 600_000);
    assert!(state.ledger.audit().mismatches.is_empty());
}
