//! Deterministic multi-validator simulation: four validators on a simulated
//! network must finalize blocks and agree on the state hash at every height.
//! Runs on Commonware's deterministic runtime, so a failure is reproducible
//! from its seed.

use crate::{
    application::{Application, HashChain, StateMachine},
    engine::{self, Engine},
    types::{Digest, PrivateKey, PublicKey},
};
use bytes::Bytes;
use commonware_consensus::marshal::resolver::p2p as resolver;
use commonware_consensus::types::Height;
use commonware_cryptography::Signer;
use commonware_p2p::simulated::{self, Link, Network};
use commonware_parallel::Sequential;
use commonware_runtime::{deterministic, Clock, Quota, Runner as _, Supervisor as _};
use commonware_utils::NZU64;
use commonware_utils::{ordered::Set, probability, NZUsize, NZU32};
use std::{collections::BTreeSet, time::Duration};

const QUOTA: Quota = Quota::per_second(NZU32!(u32::MAX));

fn run(seed: u64, validators: u64, target_height: u64, link: Link) -> String {
    run_with(
        seed,
        validators,
        target_height,
        link,
        None,
        |_| HashChain::default(),
        |m: &HashChain, h| m.hash_at(h),
    )
}

/// A state machine that rotates the validator set every epoch: epoch `e`
/// excludes node `e % n`, so every node is a non-member at some point.
#[derive(Clone)]
struct RotatingChain {
    inner: HashChain,
    keys: Vec<[u8; 32]>,
}

impl StateMachine for RotatingChain {
    fn build(&mut self, parent_height: Height, timestamp: u64) -> Bytes {
        self.inner.build(parent_height, timestamp)
    }
    fn check(&self, payload: &[u8]) -> bool {
        self.inner.check(payload)
    }
    fn apply(&mut self, height: Height, timestamp: u64, payload: &[u8]) -> Digest {
        self.inner.apply(height, timestamp, payload)
    }
    fn tip(&self) -> (Height, Digest) {
        self.inner.tip()
    }
    fn validators(&self, epoch: u64) -> Option<Vec<[u8; 32]>> {
        let n = self.keys.len() as u64;
        let skip = (epoch % n) as usize;
        Some(
            self.keys
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, k)| *k)
                .collect(),
        )
    }
}

fn run_with<M: StateMachine + Clone>(
    seed: u64,
    validators: u64,
    target_height: u64,
    link: Link,
    blocks_per_epoch: Option<u64>,
    make: impl Fn(&[PublicKey]) -> M,
    hash_at: impl Fn(&M, u64) -> Option<Digest>,
) -> String {
    let runner = deterministic::Runner::new(
        deterministic::Config::new()
            .with_seed(seed)
            .with_timeout(Some(Duration::from_secs(120))),
    );
    runner.start(|context| async move {
        let signers: Vec<PrivateKey> = (0..validators).map(PrivateKey::from_seed).collect();
        let pks: Vec<PublicKey> = signers.iter().map(|s| s.public_key()).collect();
        let participants: Set<PublicKey> = Set::from_iter_dedup(pks.clone());

        let (network, oracle) = Network::new_with_peers(
            context.child("network"),
            simulated::Config {
                max_size: 1024 * 1024,
                max_peers_per_set: NZUsize!(pks.len()),
                disconnect_on_block: true,
                tracked_peer_sets: NZUsize!(4),
            },
            pks.clone(),
        )
        .await;
        network.start();
        for a in &pks {
            for b in &pks {
                if a != b {
                    oracle
                        .add_link(a.clone(), b.clone(), link.clone())
                        .await
                        .unwrap();
                }
            }
        }

        let machine = make(&pks);
        let mut apps = Vec::new();
        for (i, signer) in signers.into_iter().enumerate() {
            let pk = signer.public_key();
            let control = oracle.control(pk.clone());
            let votes = control.register(0, QUOTA).await.unwrap();
            let certificates = control.register(1, QUOTA).await.unwrap();
            let res = control.register(2, QUOTA).await.unwrap();
            let broadcast = control.register(3, QUOTA).await.unwrap();
            let backfill = control.register(4, QUOTA).await.unwrap();

            let ctx = context.child("validator").with_attribute("id", i);
            let app = Application::new(machine.clone());
            let mut cfg = engine::devnet_timings(
                oracle.control(pk.clone()),
                oracle.manager(),
                oracle.manager(),
                signer,
                participants.clone(),
                format!("v{i}"),
                Sequential,
            );
            if let Some(n) = blocks_per_epoch {
                cfg.blocks_per_epoch = NZU64!(n);
            }
            let engine = Engine::new(ctx.child("engine"), app.clone(), cfg).await;
            let backfill = resolver::init(
                ctx.child("backfill"),
                resolver::Config {
                    public_key: pk.clone(),
                    peer_provider: oracle.manager(),
                    blocker: oracle.control(pk),
                    mailbox_size: NZUsize!(1024),
                    timeout: Duration::from_secs(2),
                    fetch_retry_timeout: Duration::from_millis(100),
                    priority_requests: false,
                    priority_responses: false,
                },
                backfill,
            );
            engine.start(votes, certificates, res, broadcast, backfill);
            apps.push(app);
        }

        loop {
            context.sleep(Duration::from_millis(100)).await;
            if apps.iter().all(|a| a.tip().0.get() >= target_height) {
                break;
            }
        }

        // Every validator must hold the same state hash at every height.
        for h in 1..=target_height {
            let hashes: BTreeSet<_> = apps
                .iter()
                .map(|a| a.with(|m| hash_at(m, h).expect("applied")))
                .collect();
            assert_eq!(hashes.len(), 1, "state diverged at height {h}: {hashes:?}");
        }
        // The deterministic runtime's audit digest: same seed => same run.
        context.auditor().state()
    })
}

/// A chain that paces its blocks and checks, inside `apply`, that every
/// block respects the interval it asked for (a violation panics the
/// deterministic run).
#[derive(Clone)]
struct PacedChain {
    inner: HashChain,
    busy_ms: u64,
    idle_ms: u64,
    pending: bool,
    last_ts: u64,
}

impl StateMachine for PacedChain {
    fn build(&mut self, parent_height: Height, timestamp: u64) -> Bytes {
        self.inner.build(parent_height, timestamp)
    }
    fn check(&self, payload: &[u8]) -> bool {
        self.inner.check(payload)
    }
    fn apply(&mut self, height: Height, timestamp: u64, payload: &[u8]) -> Digest {
        if height.get() > 1 {
            let expected = if self.pending {
                self.busy_ms
            } else {
                self.idle_ms
            };
            assert!(
                timestamp >= self.last_ts + expected,
                "block {height} came {} ms after its parent, pacing asked for {expected} ms",
                timestamp - self.last_ts
            );
        }
        self.last_ts = timestamp;
        self.inner.apply(height, timestamp, payload)
    }
    fn tip(&self) -> (Height, Digest) {
        self.inner.tip()
    }
    fn block_intervals_ms(&self) -> (u64, u64) {
        (self.busy_ms, self.idle_ms)
    }
    fn has_pending(&self) -> bool {
        self.pending
    }
}

#[test]
fn paced_timeouts_keep_the_simplex_ordering_for_any_interval() {
    use crate::epochs::{paced_timings, Timings};
    let base = Timings {
        leader_timeout: Duration::from_millis(500),
        certification_timeout: Duration::from_millis(1_000),
        nullify_retry: Duration::from_secs(5),
        fetch_timeout: Duration::from_secs(1),
        view_retention: commonware_consensus::types::ViewDelta::new(32),
        skip_timeout: Duration::from_secs(6),
        mailbox_size: 1024,
        replay_buffer: NZUsize!(1024),
        write_buffer: NZUsize!(1024),
    };
    for ms in [0u64, 200, 500, 5_000, 60_000] {
        let t = paced_timings(&base, Duration::from_millis(ms));
        assert!(t.leader_timeout > Duration::ZERO);
        assert!(t.certification_timeout > t.leader_timeout, "{ms}");
        assert!(t.skip_timeout > t.certification_timeout, "{ms}");
        assert!(t.skip_timeout > t.nullify_retry, "{ms}");
    }
}

#[test]
fn busy_chain_paces_blocks_to_the_min_interval() {
    let make = |_: &[PublicKey]| PacedChain {
        inner: HashChain::default(),
        busy_ms: 200,
        idle_ms: 900,
        pending: true,
        last_ts: 0,
    };
    run_with(7, 4, 12, fast_link(), None, make, |m: &PacedChain, h| {
        m.inner.hash_at(h)
    });
}

#[test]
fn idle_chain_waits_for_the_idle_interval() {
    let make = |_: &[PublicKey]| PacedChain {
        inner: HashChain::default(),
        busy_ms: 200,
        idle_ms: 900,
        pending: false,
        last_ts: 0,
    };
    run_with(8, 4, 8, fast_link(), None, make, |m: &PacedChain, h| {
        m.inner.hash_at(h)
    });
}

fn fast_link() -> Link {
    Link {
        latency: Duration::from_millis(10),
        jitter: Duration::from_millis(1),
        success_rate: probability!(1.0),
    }
}

#[test]
fn four_validators_agree_on_state_hash() {
    run(1, 4, 20, fast_link());
}

#[test]
fn lossy_links_still_finalize_and_agree() {
    let lossy = Link {
        latency: Duration::from_millis(40),
        jitter: Duration::from_millis(20),
        success_rate: probability!(0.9),
    };
    run(2, 4, 10, lossy);
}

#[test]
fn same_seed_is_byte_identical() {
    let a = run(7, 4, 8, fast_link());
    let b = run(7, 4, 8, fast_link());
    assert_eq!(a, b);
}

#[test]
fn validator_set_rotates_every_epoch() {
    // 5 nodes, 4 validators per epoch, 6-block epochs: by height 30 the set
    // has rotated five times and every node has sat out one epoch.
    run_with(
        11,
        5,
        30,
        fast_link(),
        Some(6),
        |pks| RotatingChain {
            inner: HashChain::default(),
            keys: pks
                .iter()
                .map(|p| {
                    let b: &[u8] = p.as_ref();
                    let mut k = [0u8; 32];
                    k.copy_from_slice(b);
                    k
                })
                .collect(),
        },
        |m, h| m.inner.hash_at(h),
    );
}
