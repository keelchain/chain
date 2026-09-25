//! End-to-end VM tests for cross-chain vaults: deposit addresses,
//! observer-quorum deposits (with and without light-client proofs), holds,
//! withdrawals, batching, outbound confirmation, fee reports, vault
//! registration and the reserves invariant.
#![allow(clippy::unwrap_used)]

use bitcoin::{
    block::{Header, Version},
    blockdata::constants::genesis_block,
    consensus::serialize,
    hashes::{sha256d, Hash},
    merkle_tree::PartialMerkleTree,
    Network, TxMerkleNode, Txid,
};
use keel_actions::{
    Action, Chain, DepositObservation, OutboundObservation, Proof, SignedAction, VaultRegistration,
    Withdraw, CHAIN_ID_DEVNET,
};
use keel_crypto::Keypair;
use keel_ledger::{AccountKey, Record, TxType};
use keel_types::{Address, Asset};
use keel_vm::{
    apply_block,
    genesis::GenesisValidator,
    modules::{tokens, vaults},
    BlockContext, Event, Genesis, State, VmError,
};

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

/// Genesis with two funded users and three validators that double as
/// observers (threshold 2) and arbitrators.
fn setup() -> (State, Actor, Actor, Vec<Actor>) {
    let alice = Actor::new(1);
    let bob = Actor::new(2);
    let observers: Vec<Actor> = (100..103).map(Actor::new).collect();
    let validators = observers
        .iter()
        .map(|o| GenesisValidator {
            address: o.addr(),
            consensus_key: o.addr().0,
            bond: 0,
        })
        .collect();
    let g = Genesis::devnet(CHAIN_ID_DEVNET, &[alice.addr(), bob.addr()], validators);
    (g.build(), alice, bob, observers)
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
fn tron_usdt() -> Asset {
    Asset::new("TRON.USDT")
}
fn sys(asset: Asset, t: &str) -> AccountKey {
    AccountKey::new(Address::SYSTEM, asset, t).unwrap()
}
fn user(a: Address, asset: Asset, t: &str) -> AccountKey {
    AccountKey::new(a, asset, t).unwrap()
}
fn ok(state: &mut State, height: u64, actions: &[SignedAction]) -> Vec<keel_vm::Receipt> {
    let (r, _) = apply_block(state, &ctx(height), actions);
    for x in &r {
        assert!(x.ok, "action {} failed: {:?}", x.index, x.error);
    }
    r
}
fn audit_clean(state: &State) {
    assert!(state.ledger.audit().mismatches.is_empty());
}

fn tron_obs(
    deposit_index: u64,
    amount: u128,
    tx: u8,
    external: u64,
    tip: u64,
) -> DepositObservation {
    DepositObservation {
        chain: Chain::Tron,
        asset: tron_usdt(),
        tx_hash: [tx; 32],
        index: 0,
        deposit_index,
        amount,
        external_height: external,
        tip_height: tip,
        proof: Proof::None,
    }
}

// ---- Bitcoin proof helpers (regtest difficulty, mined in the test) ----

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

/// A chain of `n` regtest headers after genesis; the first one contains
/// `txids`, with the proof matching index 1.
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
fn deposit_addresses_are_sequential_and_stable() {
    let (mut state, mut alice, mut bob, _) = setup();
    let r = ok(
        &mut state,
        1,
        &[
            alice.act(Action::RequestDepositAddress {
                chain: Chain::Bitcoin,
            }),
            bob.act(Action::RequestDepositAddress {
                chain: Chain::Bitcoin,
            }),
            alice.act(Action::RequestDepositAddress { chain: Chain::Tron }),
            alice.act(Action::RequestDepositAddress {
                chain: Chain::Bitcoin,
            }),
        ],
    );
    assert_eq!(
        state.vaults.deposit_index(alice.addr(), Chain::Bitcoin),
        Some(1)
    );
    assert_eq!(
        state.vaults.deposit_index(bob.addr(), Chain::Bitcoin),
        Some(2)
    );
    assert_eq!(
        state.vaults.deposit_index(alice.addr(), Chain::Tron),
        Some(1)
    );
    // Asking again returns the same index without allocating a new one.
    assert_eq!(
        r[3].events,
        vec![Event::DepositAddressAssigned {
            owner: alice.addr(),
            chain: "BTC".into(),
            index: 1
        }]
    );
    assert_eq!(state.vaults.next_deposit_index[&Chain::Bitcoin], 3);
    assert_eq!(state.vaults.deposit_owner[&(Chain::Bitcoin, 2)], bob.addr());
    audit_clean(&state);
}

#[test]
fn tron_deposit_needs_observer_quorum_depth_and_dedups() {
    let (mut state, mut alice, mut bob, mut obs) = setup();
    ok(
        &mut state,
        1,
        &[alice.act(Action::RequestDepositAddress { chain: Chain::Tron })],
    );
    let before = tokens::balance(&state, alice.addr(), &tron_usdt());
    // A user cannot observe; an observer needs enough depth; unknown index refused.
    let (r, _) = apply_block(
        &mut state,
        &ctx(2),
        &[
            bob.act(Action::ObserveDeposit(tron_obs(1, 5_000_000, 7, 100, 200))),
            obs[0].act(Action::ObserveDeposit(tron_obs(1, 5_000_000, 7, 100, 110))),
            obs[0].act(Action::ObserveDeposit(tron_obs(9, 5_000_000, 7, 100, 200))),
        ],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    assert!(
        matches!(r[1].error, Some(VmError::Invalid(ref m)) if m.contains("confirmations")),
        "{:?}",
        r[1].error
    );
    assert!(matches!(r[2].error, Some(VmError::NotFound(_))));
    // First vote: pending, nothing credited. Same observer twice: refused.
    let (r, _) = apply_block(
        &mut state,
        &ctx(3),
        &[
            obs[0].act(Action::ObserveDeposit(tron_obs(1, 5_000_000, 7, 100, 200))),
            obs[0].act(Action::ObserveDeposit(tron_obs(1, 5_000_000, 7, 100, 200))),
        ],
    );
    assert!(r[0].ok, "{:?}", r[0].error);
    assert_eq!(
        r[0].events,
        vec![Event::DepositObserved {
            chain: "TRON".into(),
            tx_hash: "07".repeat(32),
            votes: 1
        }]
    );
    assert!(matches!(r[1].error, Some(VmError::Invalid(ref m)) if m.contains("already voted")));
    assert_eq!(tokens::balance(&state, alice.addr(), &tron_usdt()), before);
    // Second observer reaches the threshold of 2: credited from the vault reserves.
    let r = ok(
        &mut state,
        4,
        &[obs[1].act(Action::ObserveDeposit(tron_obs(1, 5_000_000, 7, 100, 200)))],
    );
    assert_eq!(
        r[0].events[1],
        Event::DepositCredited {
            owner: alice.addr(),
            asset: tron_usdt(),
            amount: 5_000_000
        }
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &tron_usdt()),
        before + 5_000_000
    );
    assert_eq!(
        state.ledger.balance(&sys(tron_usdt(), "vault_asset")),
        5_000_000
    );
    // Late third vote and a replay of the same tx are refused.
    let (r, _) = apply_block(
        &mut state,
        &ctx(5),
        &[
            obs[2].act(Action::ObserveDeposit(tron_obs(1, 5_000_000, 7, 100, 200))),
            obs[2].act(Action::ObserveDeposit(tron_obs(1, 6_000_000, 7, 100, 200))),
        ],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(ref m)) if m.contains("already credited")));
    assert!(matches!(r[1].error, Some(VmError::Invalid(ref m)) if m.contains("already credited")));
    assert_eq!(
        tokens::balance(&state, alice.addr(), &tron_usdt()),
        before + 5_000_000
    );
    // A mismatching asset/chain pair is refused.
    let (r, _) = apply_block(
        &mut state,
        &ctx(6),
        &[obs[0].act(Action::ObserveDeposit(DepositObservation {
            asset: btc(),
            ..tron_obs(1, 1, 8, 100, 200)
        }))],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(ref m)) if m.contains("not custodied")));
    audit_clean(&state);
}

#[test]
fn bitcoin_deposit_requires_valid_spv_proof() {
    let (mut state, mut alice, _bob, mut obs) = setup();
    ok(
        &mut state,
        1,
        &[alice.act(Action::RequestDepositAddress {
            chain: Chain::Bitcoin,
        })],
    );
    let txids = vec![txid(1), txid(2), txid(3)];
    let (headers, proof) = btc_chain(4, &txids); // containing block + 3 on top
    let deposit = txid(2).to_byte_array();
    let obs_ok = |proof: Proof, index: u32| DepositObservation {
        chain: Chain::Bitcoin,
        asset: btc(),
        tx_hash: deposit,
        index,
        deposit_index: 1,
        amount: 1_000_000,
        external_height: 100,
        tip_height: 103,
        proof,
    };
    let before = tokens::balance(&state, alice.addr(), &btc());
    // No proof, tampered proof, proof for another index: all refused.
    let mut tampered = proof.clone();
    tampered[5] ^= 0x01;
    let mut wrong_link = headers.clone();
    wrong_link.swap(1, 2);
    let (r, _) = apply_block(
        &mut state,
        &ctx(2),
        &[
            obs[0].act(Action::ObserveDeposit(obs_ok(Proof::None, 1))),
            obs[0].act(Action::ObserveDeposit(obs_ok(
                Proof::Bitcoin {
                    headers: headers.clone(),
                    merkle_proof: tampered,
                    tx_index: 1,
                },
                1,
            ))),
            obs[0].act(Action::ObserveDeposit(obs_ok(
                Proof::Bitcoin {
                    headers: wrong_link,
                    merkle_proof: proof.clone(),
                    tx_index: 1,
                },
                1,
            ))),
            obs[0].act(Action::ObserveDeposit(obs_ok(
                Proof::Bitcoin {
                    headers: headers.clone(),
                    merkle_proof: proof.clone(),
                    tx_index: 0,
                },
                0,
            ))),
            obs[0].act(Action::ObserveDeposit(obs_ok(
                Proof::Bitcoin {
                    headers: headers[..2].to_vec(),
                    merkle_proof: proof.clone(),
                    tx_index: 1,
                },
                1,
            ))),
        ],
    );
    for x in &r {
        assert!(
            matches!(x.error, Some(VmError::Invalid(_))),
            "action {} should be invalid: {:?}",
            x.index,
            x.error
        );
    }
    assert!(state.vaults.pending.is_empty());
    // A valid proof from two observers credits.
    let good = Proof::Bitcoin {
        headers: headers.clone(),
        merkle_proof: proof.clone(),
        tx_index: 1,
    };
    ok(
        &mut state,
        3,
        &[
            obs[0].act(Action::ObserveDeposit(obs_ok(good.clone(), 1))),
            obs[1].act(Action::ObserveDeposit(obs_ok(good.clone(), 1))),
        ],
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        before + 1_000_000
    );
    // The same transaction paying a second vault output (vout 2) is a
    // separate credit under the same merkle proof; vout 1 stays deduped.
    let r = ok(
        &mut state,
        4,
        &[
            obs[0].act(Action::ObserveDeposit(obs_ok(good.clone(), 2))),
            obs[1].act(Action::ObserveDeposit(obs_ok(good.clone(), 2))),
        ],
    );
    assert!(
        r.iter().all(|x| x.ok),
        "{:?}",
        r.iter().map(|x| &x.error).collect::<Vec<_>>()
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        before + 2_000_000
    );
    let r = keel_vm::apply_block(
        &mut state,
        &ctx(5),
        &[obs[2].act(Action::ObserveDeposit(obs_ok(good, 1)))],
    )
    .0;
    assert!(!r[0].ok, "vout 1 must stay credited once");
    audit_clean(&state);
}

#[test]
fn large_deposits_are_held_then_released() {
    let (mut state, mut alice, _bob, mut obs) = setup();
    // Give BTC a price: 60,000 KUSD per BTC, so 1 BTC is a large deposit.
    state.markets.pairs.get_mut("BTC-KUSD").unwrap().last_price = Some(60_000_000_000);
    assert_eq!(
        tokens::usd_value(&state, &btc(), 100_000_000),
        Some(60_000_000_000)
    );
    ok(
        &mut state,
        1,
        &[alice.act(Action::RequestDepositAddress { chain: Chain::Tron })],
    );
    // Use Tron USDT so no proof is needed: 60,000 USDT... TRON.USDT has no
    // price, so use BTC via Tron? No: price is per asset. Register a Tron
    // deposit of a priced asset is impossible; instead price TRON.USDT.
    keel_vm::modules::markets::list_pair(
        &mut state,
        keel_vm::modules::markets::default_pair("USDT-KUSD", tron_usdt(), Asset::new("KUSD"), 6, 6),
    );
    state.markets.pairs.get_mut("USDT-KUSD").unwrap().last_price = Some(1_000_000);
    let before = tokens::balance(&state, alice.addr(), &tron_usdt());
    let big = 60_000_000_000u128; // 60,000 USDT
    let r = ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::ObserveDeposit(tron_obs(1, big, 9, 100, 200))),
            obs[1].act(Action::ObserveDeposit(tron_obs(1, big, 9, 100, 200))),
        ],
    );
    let release_height = 2 + state.params.large_deposit_delay_blocks;
    assert_eq!(
        r[1].events[1],
        Event::DepositHeld {
            owner: alice.addr(),
            asset: tron_usdt(),
            amount: big,
            release_height
        }
    );
    assert_eq!(tokens::balance(&state, alice.addr(), &tron_usdt()), before);
    assert_eq!(
        state
            .ledger
            .balance(&user(alice.addr(), tron_usdt(), "screening_hold")),
        big as i128
    );
    // Still a user liability while held, so reserves cover it.
    assert_eq!(state.ledger.user_liabilities(&tron_usdt()), before + big);
    // Not released before the delay; released at the delay.
    let (_, end) = apply_block(&mut state, &ctx(release_height - 1), &[]);
    assert!(end.is_empty());
    let (_, end) = apply_block(&mut state, &ctx(release_height), &[]);
    assert_eq!(
        end,
        vec![Event::DepositReleased {
            owner: alice.addr(),
            asset: tron_usdt(),
            amount: big
        }]
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &tron_usdt()),
        before + big
    );
    assert_eq!(
        state
            .ledger
            .balance(&user(alice.addr(), tron_usdt(), "screening_hold")),
        0
    );
    assert!(state.vaults.held.is_empty());
    audit_clean(&state);
}

fn register_btc_vault(state: &mut State, obs: &mut [Actor], epoch: u64) {
    let signers: Vec<Address> = obs.iter().map(|o| o.addr()).collect();
    ok(
        state,
        1,
        &[obs[0].act(Action::RegisterVault(VaultRegistration {
            chain: Chain::Bitcoin,
            epoch,
            public_key: vec![2u8; 33],
            chain_code: Some([1u8; 32]),
            signers,
            threshold: 2,
        }))],
    );
}

#[test]
fn withdraw_locks_escrow_validates_address_and_fee() {
    let (mut state, mut alice, _bob, mut obs) = setup();
    let good_addr = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
    // No vault yet: refused.
    let (r, _) = apply_block(
        &mut state,
        &ctx(1),
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to: good_addr.clone(),
            amount: 1_000_000,
        }))],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(ref m)) if m.contains("no vault")));
    register_btc_vault(&mut state, &mut obs, 1);
    // Fee reports: median of 5, 10, 100 is 10 sat/vB -> 2,000 sats for 200 vB.
    ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 5,
            }),
            obs[1].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 100,
            }),
            obs[2].act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 10,
            }),
        ],
    );
    assert_eq!(state.vaults.fee_rate(Chain::Bitcoin), 10);
    let start = tokens::balance(&state, alice.addr(), &btc());
    let (r, _) = apply_block(
        &mut state,
        &ctx(3),
        &[
            alice.act(Action::Withdraw(Withdraw {
                asset: btc(),
                to: "0xdeadbeef".into(),
                amount: 1_000_000,
            })),
            alice.act(Action::Withdraw(Withdraw {
                asset: btc(),
                to: good_addr.clone(),
                amount: 0,
            })),
            alice.act(Action::Withdraw(Withdraw {
                asset: Asset::new("KUSD"),
                to: good_addr.clone(),
                amount: 5,
            })),
            alice.act(Action::Withdraw(Withdraw {
                asset: btc(),
                to: good_addr.clone(),
                amount: 1_000_000,
            })),
        ],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(ref m)) if m.contains("destination")));
    assert!(matches!(r[1].error, Some(VmError::Invalid(_))));
    assert!(matches!(r[2].error, Some(VmError::Invalid(ref m)) if m.contains("not a vault asset")));
    assert!(r[3].ok, "{:?}", r[3].error);
    assert_eq!(
        r[3].events,
        vec![Event::WithdrawalQueued {
            outbound_id: 0,
            owner: alice.addr(),
            asset: btc(),
            amount: 1_000_000,
            to: good_addr.clone()
        }]
    );
    // Amount + network fee estimate locked; reserves untouched until confirmed.
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        start - 1_002_000
    );
    assert_eq!(
        state
            .ledger
            .balance(&user(alice.addr(), btc(), "sendout_escrow")),
        1_002_000
    );
    let ob = &state.vaults.outbounds[&0];
    assert_eq!(
        (ob.fee_asset.clone(), ob.fee_estimate, ob.status),
        (btc(), 2_000, vaults::OutboundStatus::Queued)
    );
    // Halted asset refuses withdrawals; resume allows them again.
    vaults::halt_outbounds(&mut state, &btc());
    let (r, _) = apply_block(
        &mut state,
        &ctx(4),
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to: good_addr.clone(),
            amount: 1_000,
        }))],
    );
    assert!(matches!(r[0].error, Some(VmError::Paused(_))));
    vaults::resume_outbounds(&mut state, &btc());
    ok(
        &mut state,
        5,
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to: good_addr,
            amount: 1_000,
        }))],
    );
    assert_eq!(state.vaults.outbounds.len(), 2);
    audit_clean(&state);
}

#[test]
fn outbounds_batch_confirm_and_fail_refund() {
    let (mut state, mut alice, _bob, mut obs) = setup();
    register_btc_vault(&mut state, &mut obs, 1);
    ok(
        &mut state,
        2,
        &[obs[0].act(Action::ReportNetworkFee {
            chain: Chain::Bitcoin,
            fee_rate: 10,
        })],
    );
    let to = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
    let start = tokens::balance(&state, alice.addr(), &btc());
    let reserves = state.ledger.balance(&sys(btc(), "vault_asset"));
    ok(
        &mut state,
        3,
        &[
            alice.act(Action::Withdraw(Withdraw {
                asset: btc(),
                to: to.clone(),
                amount: 1_000_000,
            })),
            alice.act(Action::Withdraw(Withdraw {
                asset: btc(),
                to,
                amount: 500_000,
            })),
        ],
    );
    // Confirmation before batching is refused; batching happens on the interval.
    let interval = state.params.outbound_batch_interval_blocks;
    let observation = |id, success, fee_paid| OutboundObservation {
        outbound_id: id,
        tx_hash: [0xaa; 32],
        external_height: 300,
        tip_height: 306,
        fee_paid,
        success,
    };
    let (r, _) = apply_block(
        &mut state,
        &ctx(4),
        &[obs[0].act(Action::ObserveOutbound(observation(0, true, 1_500)))],
    );
    assert!(matches!(r[0].error, Some(VmError::Invalid(ref m)) if m.contains("not awaiting")));
    let (_, end) = apply_block(&mut state, &ctx(interval - 1), &[]);
    assert!(end.is_empty());
    let (_, end) = apply_block(&mut state, &ctx(interval), &[]);
    assert_eq!(
        end,
        vec![
            Event::OutboundBatched {
                outbound_id: 0,
                chain: "BTC".into()
            },
            Event::OutboundBatched {
                outbound_id: 1,
                chain: "BTC".into()
            }
        ]
    );
    assert_eq!(state.vaults.batches[&0].outbound_ids, vec![0, 1]);
    assert_eq!(state.vaults.outbounds[&1].batch_id, Some(0));
    // One vote: nothing; second vote confirms #0: amount + paid fee leave reserves, overpayment refunded.
    let r = ok(
        &mut state,
        interval + 1,
        &[obs[0].act(Action::ObserveOutbound(observation(0, true, 1_500)))],
    );
    assert!(r[0].events.is_empty());
    let r = ok(
        &mut state,
        interval + 2,
        &[obs[1].act(Action::ObserveOutbound(observation(0, true, 1_500)))],
    );
    assert_eq!(
        r[0].events,
        vec![Event::OutboundConfirmed {
            outbound_id: 0,
            tx_hash: "aa".repeat(32)
        }]
    );
    assert_eq!(
        state.vaults.outbounds[&0].status,
        vaults::OutboundStatus::Confirmed
    );
    assert_eq!(
        state.ledger.balance(&sys(btc(), "vault_asset")),
        reserves - 1_001_500
    );
    assert_eq!(
        state
            .ledger
            .balance(&user(alice.addr(), btc(), "sendout_escrow")),
        502_000
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        start - 1_504_000 + 500
    );
    // #1 fails on chain: everything back to the owner, reserves untouched.
    let r = ok(
        &mut state,
        interval + 3,
        &[
            obs[0].act(Action::ObserveOutbound(observation(1, false, 0))),
            obs[2].act(Action::ObserveOutbound(observation(1, false, 0))),
        ],
    );
    assert_eq!(
        r[1].events,
        vec![Event::OutboundFailed {
            outbound_id: 1,
            refunded: 500_000
        }]
    );
    assert_eq!(
        state
            .ledger
            .balance(&user(alice.addr(), btc(), "sendout_escrow")),
        0
    );
    assert_eq!(
        tokens::balance(&state, alice.addr(), &btc()),
        start - 1_001_500
    );
    assert_eq!(
        state.ledger.balance(&sys(btc(), "vault_asset")),
        reserves - 1_001_500
    );
    // Reserves still cover liabilities.
    assert!(state.ledger.system_reserves(&btc()) as u128 >= state.ledger.user_liabilities(&btc()));
    audit_clean(&state);
}

#[test]
fn network_fee_median_and_vault_registration_rules() {
    let (mut state, mut alice, _bob, mut obs) = setup();
    let signers: Vec<Address> = obs.iter().map(|o| o.addr()).collect();
    let reg = |epoch, key_len, threshold| VaultRegistration {
        chain: Chain::Bitcoin,
        epoch,
        public_key: vec![3u8; key_len],
        chain_code: None,
        signers: signers.clone(),
        threshold,
    };
    let (r, _) = apply_block(
        &mut state,
        &ctx(1),
        &[
            alice.act(Action::RegisterVault(reg(1, 33, 2))),
            alice.act(Action::ReportNetworkFee {
                chain: Chain::Bitcoin,
                fee_rate: 1,
            }),
            obs[0].act(Action::RegisterVault(reg(1, 32, 2))),
            obs[0].act(Action::RegisterVault(reg(1, 33, 4))),
            obs[0].act(Action::RegisterVault(reg(1, 33, 2))),
            obs[1].act(Action::RegisterVault(reg(1, 33, 2))),
            obs[1].act(Action::RegisterVault(reg(2, 33, 2))),
        ],
    );
    assert_eq!(r[0].error, Some(VmError::Unauthorized));
    assert_eq!(r[1].error, Some(VmError::Unauthorized));
    assert!(matches!(r[2].error, Some(VmError::Invalid(ref m)) if m.contains("key length")));
    assert!(matches!(r[3].error, Some(VmError::Invalid(ref m)) if m.contains("threshold")));
    assert!(r[4].ok);
    assert!(matches!(r[5].error, Some(VmError::Invalid(ref m)) if m.contains("epoch")));
    assert!(r[6].ok);
    assert_eq!(state.vaults.active_vault(Chain::Bitcoin).unwrap().epoch, 2);
    assert_eq!(state.vaults.vaults.len(), 2);
    // Fee median: latest report per observer wins.
    ok(
        &mut state,
        2,
        &[
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Tron,
                fee_rate: 100,
            }),
            obs[1].act(Action::ReportNetworkFee {
                chain: Chain::Tron,
                fee_rate: 300,
            }),
            obs[2].act(Action::ReportNetworkFee {
                chain: Chain::Tron,
                fee_rate: 200,
            }),
            obs[0].act(Action::ReportNetworkFee {
                chain: Chain::Tron,
                fee_rate: 400,
            }),
        ],
    );
    assert_eq!(state.vaults.fee_rate(Chain::Tron), 300);
    assert_eq!(state.vaults.fee_rate(Chain::Ethereum), 0);
    audit_clean(&state);
}

#[test]
fn reserve_breach_halts_outbounds() {
    let (mut state, mut alice, _bob, mut obs) = setup();
    register_btc_vault(&mut state, &mut obs, 1);
    let to = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
    ok(
        &mut state,
        2,
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to: to.clone(),
            amount: 1_000,
        }))],
    );
    // A liability nothing backs: the platform owes 1 sat it does not hold.
    state
        .ledger
        .post(
            "oops",
            TxType::SystemFundsExpense,
            None,
            None,
            vec![
                Record::debit(sys(btc(), "system_funds"), 1),
                Record::credit(user(alice.addr(), btc(), "deposit"), 1),
            ],
        )
        .unwrap();
    let liabilities = state.ledger.user_liabilities(&btc());
    let reserves = state.ledger.system_reserves(&btc()) as u128;
    assert!(reserves < liabilities);
    let (_, end) = apply_block(&mut state, &ctx(3), &[]);
    assert_eq!(
        end,
        vec![Event::InvariantBreached {
            asset: btc(),
            reserves,
            liabilities
        }]
    );
    assert!(state.vaults.halted.contains(&btc()));
    // Withdrawals refused and queued outbounds are not batched while halted.
    let (r, end) = apply_block(
        &mut state,
        &ctx(20),
        &[alice.act(Action::Withdraw(Withdraw {
            asset: btc(),
            to,
            amount: 1_000,
        }))],
    );
    assert!(matches!(r[0].error, Some(VmError::Paused(_))));
    assert!(end
        .iter()
        .all(|e| !matches!(e, Event::OutboundBatched { .. })));
    assert_eq!(
        state.vaults.outbounds[&0].status,
        vaults::OutboundStatus::Queued
    );
    audit_clean(&state);
}
