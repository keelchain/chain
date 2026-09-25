//! Bridges consensus to the deterministic state machine.
//!
//! Division of labour (docs/plan.md §1):
//! - `propose` asks the state machine for a payload on top of the parent.
//! - `verify` checks structure only (timestamps, well-formed payload). It
//!   never executes: a block with actions that fail is still a valid block,
//!   the actions just get failure receipts. That keeps verification
//!   stateless and cheap, and finalization is where execution happens.
//! - `report` receives finalized blocks in order (at-least-once, so it
//!   ignores anything at or below the applied height) and applies them.

use crate::{
    block::Block,
    types::{Context, Digest, Hasher, PrivateKey, Scheme, EPOCH},
};
use bytes::Bytes;
use commonware_actor::Feedback;
use commonware_consensus::{
    marshal::{ancestry::Ancestry, Update},
    types::{Height, Round, View},
    Application as ConsensusApplication, Heightable, Reporter,
};
use commonware_cryptography::{Digest as _, Digestible, Hasher as _, Signer};
use commonware_runtime::{Clock, Metrics, Spawner};
use commonware_utils::{Acknowledgement, SystemTimeExt};
use futures::StreamExt as _;
use rand_core::Rng;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tracing::{info, warn};

/// Fixed consensus cutoff for block timestamps: 2200-01-01T00:00:00Z.
const MAX_BLOCK_TIMESTAMP_MS: u64 = 7_258_118_400_000;

/// What the VM must provide. All three run on the consensus thread and must
/// be deterministic and quick; heavy work belongs in the mempool.
pub trait StateMachine: Send + 'static {
    /// Payload for a new block on top of `parent_height` at `timestamp`.
    fn build(&mut self, parent_height: Height, timestamp: u64) -> Bytes;

    /// Structural validity of a payload. Must not depend on state.
    fn check(&self, payload: &[u8]) -> bool;

    /// Apply a finalized block. Called exactly once per height, in order.
    /// Returns the state hash after the block.
    fn apply(&mut self, height: Height, timestamp: u64, payload: &[u8]) -> Digest;

    /// Last applied height and state hash (genesis is height 0).
    fn tip(&self) -> (Height, Digest);

    /// Consensus keys (ed25519, 32 bytes) of the validators of `epoch`, as
    /// decided by the state at the boundary block before it. `None` keeps
    /// the genesis validator set (single-epoch chains, tests).
    fn validators(&self, epoch: u64) -> Option<Vec<[u8; 32]>> {
        let _ = epoch;
        None
    }

    /// Block pacing from state: `(busy_ms, idle_ms)`. A proposer waits at
    /// least `busy_ms` after the parent block, and up to `idle_ms` while it
    /// has nothing to include. `(0, 0)` proposes as fast as consensus runs.
    fn block_intervals_ms(&self) -> (u64, u64) {
        (0, 0)
    }

    /// Whether a proposal right now would carry transactions (mempool not
    /// empty). Decides between the busy and the idle interval.
    fn has_pending(&self) -> bool {
        true
    }
}

/// A state machine that only chains hashes: the Phase 0 spike target
/// (byte-identical state across nodes after replay).
#[derive(Debug, Clone)]
pub struct HashChain {
    height: Height,
    hash: Digest,
    /// State hash after every applied height, for cross-node comparison.
    history: std::collections::BTreeMap<u64, Digest>,
}

impl Default for HashChain {
    fn default() -> Self {
        Self {
            height: Height::zero(),
            hash: Digest::EMPTY,
            history: Default::default(),
        }
    }
}

impl HashChain {
    pub fn hash_at(&self, height: u64) -> Option<Digest> {
        self.history.get(&height).copied()
    }
}

impl StateMachine for HashChain {
    fn build(&mut self, parent_height: Height, timestamp: u64) -> Bytes {
        Bytes::from(format!("keel:{}:{}", parent_height.next(), timestamp))
    }

    fn check(&self, payload: &[u8]) -> bool {
        payload.starts_with(b"keel:")
    }

    fn apply(&mut self, height: Height, timestamp: u64, payload: &[u8]) -> Digest {
        let mut h = Hasher::default();
        h.update(self.hash.as_ref());
        h.update(&height.get().to_be_bytes());
        h.update(&timestamp.to_be_bytes());
        h.update(payload);
        self.hash = h.finalize().1;
        self.height = height;
        self.history.insert(height.get(), self.hash);
        self.hash
    }

    fn tip(&self) -> (Height, Digest) {
        (self.height, self.hash)
    }
}

/// The consensus-facing application. Cheap to clone; all clones share one
/// state machine.
pub struct Application<M: StateMachine> {
    machine: Arc<Mutex<M>>,
    max_propose_delay_ms: Arc<AtomicU64>,
}

impl<M: StateMachine> Clone for Application<M> {
    fn clone(&self) -> Self {
        Self {
            machine: self.machine.clone(),
            max_propose_delay_ms: self.max_propose_delay_ms.clone(),
        }
    }
}

/// Poll period while a proposer idles waiting for transactions.
const IDLE_POLL: Duration = Duration::from_millis(50);

impl<M: StateMachine> Application<M> {
    pub fn new(machine: M) -> Self {
        Self {
            machine: Arc::new(Mutex::new(machine)),
            max_propose_delay_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Cap on the proposer's wait, set by the orchestrator per epoch from
    /// the leader timeout. Zero (the default) disables pacing.
    pub fn set_max_propose_delay(&self, d: Duration) {
        self.max_propose_delay_ms
            .store(d.as_millis() as u64, Ordering::Relaxed);
    }

    pub fn max_propose_delay(&self) -> Duration {
        Duration::from_millis(self.max_propose_delay_ms.load(Ordering::Relaxed))
    }

    /// The genesis block every node starts from. Deterministic: no clock.
    pub fn genesis() -> Block {
        let context = Context {
            round: Round::new(EPOCH, View::zero()),
            leader: PrivateKey::from_seed(0).public_key(),
            parent: (View::zero(), Digest::EMPTY),
        };
        Block::new(
            context,
            Hasher::hash(&[b"keel genesis"]),
            Height::zero(),
            0,
            Bytes::new(),
        )
    }

    pub fn tip(&self) -> (Height, Digest) {
        self.machine
            .lock()
            .map(|m| m.tip())
            .unwrap_or((Height::zero(), Digest::EMPTY))
    }

    /// Read-only access to the state machine (RPC, tests).
    pub fn with<R>(&self, f: impl FnOnce(&M) -> R) -> R {
        f(&self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, M> {
        // A poisoned lock means a panic mid-apply; the state is undefined and
        // continuing would fork. Halting is the only safe option.
        self.machine.lock().expect("state machine lock poisoned")
    }
}

impl<E, M> ConsensusApplication<E> for Application<M>
where
    E: Rng + Spawner + Metrics + Clock,
    M: StateMachine,
{
    type SigningScheme = Scheme;
    type Context = Context;
    type Block = Block;
    type Input = ();

    async fn propose(
        &mut self,
        (runtime, context): (E, Self::Context),
        mut ancestry: impl Ancestry<Self::Block>,
        _: (),
    ) -> Option<Self::Block> {
        let parent = ancestry.next().await?;
        // Pacing: wait out the busy interval, then keep waiting (up to the
        // idle interval) while there is nothing to include. Both capped by
        // the leader timeout so the view never times out on the leader.
        let cap = self.max_propose_delay_ms.load(Ordering::Relaxed);
        if cap > 0 {
            let (busy, idle) = self.with(|m| m.block_intervals_ms());
            let busy = busy.min(cap);
            let idle = idle.clamp(busy, cap);
            let busy_at = parent.timestamp.saturating_add(busy);
            let idle_at = parent.timestamp.saturating_add(idle);
            let at = |ms: u64| SystemTime::UNIX_EPOCH + Duration::from_millis(ms);
            if runtime.current().epoch_millis() < busy_at {
                runtime.sleep_until(at(busy_at)).await;
            }
            while runtime.current().epoch_millis() < idle_at && !self.with(|m| m.has_pending()) {
                let next = runtime
                    .current()
                    .epoch_millis()
                    .saturating_add(IDLE_POLL.as_millis() as u64)
                    .min(idle_at);
                runtime.sleep_until(at(next)).await;
            }
        }
        let mut now = runtime.current().epoch_millis();
        if now <= parent.timestamp {
            now = parent.timestamp.checked_add(1)?;
        }
        if now > MAX_BLOCK_TIMESTAMP_MS {
            return None;
        }
        let payload = self.lock().build(parent.height(), now);
        Some(Block::new(
            context,
            parent.digest(),
            parent.height().next(),
            now,
            payload,
        ))
    }

    async fn verify(
        &mut self,
        (runtime, _): (E, Self::Context),
        mut ancestry: impl Ancestry<Self::Block>,
    ) -> bool {
        let Some(block) = ancestry.next().await else {
            return false;
        };
        let Some(parent) = ancestry.next().await else {
            return false;
        };
        if block.timestamp <= parent.timestamp || block.timestamp > MAX_BLOCK_TIMESTAMP_MS {
            return false;
        }
        if !self.lock().check(&block.payload) {
            return false;
        }
        // Do not vote for a block from the future (clock skew): wait it out.
        let deadline = SystemTime::UNIX_EPOCH + Duration::from_millis(block.timestamp);
        runtime.sleep_until(deadline).await;
        // Height and parent-digest linkage are enforced by the marshal wrapper.
        true
    }
}

impl<M: StateMachine> Reporter for Application<M> {
    type Activity = Update<Block>;

    fn report(&mut self, update: Self::Activity) -> Feedback {
        if let Update::Block(block, ack) = update {
            let mut machine = self.lock();
            let (applied, _) = machine.tip();
            if block.height() > applied {
                if block.height() != applied.next() {
                    // Marshal guarantees gap-free delivery; a gap is a bug.
                    warn!(%applied, height = %block.height(), "finalized block out of order");
                    return Feedback::Ok;
                }
                let hash = machine.apply(block.height(), block.timestamp, &block.payload);
                info!(height = %block.height(), digest = ?block.digest(), state = ?hash, "applied");
            }
            drop(machine);
            ack.acknowledge();
        }
        Feedback::Ok
    }
}
