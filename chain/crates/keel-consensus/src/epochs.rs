//! Epoch rotation: one simplex engine per epoch, validator sets read from
//! the state machine at every boundary block (docs/plan.md Phase 2).
//!
//! Mechanics (after Commonware's reshare orchestrator, minus the DKG):
//! - Vote, certificate and resolver channels are multiplexed by epoch, so
//!   a new engine can be started without re-registering with the network.
//! - Marshal verifies certificates through [`EpochProvider`], which holds
//!   one ed25519 scheme per epoch.
//! - The orchestrator is a marshal [`Reporter`]: when the last block of
//!   epoch `E` is finalized (and, by reporter order, already applied by the
//!   application), it asks the state machine for the validators of `E+1`,
//!   registers their scheme, tracks the peer set, and starts the next
//!   engine with the boundary block as its floor.
//! - On restart it resumes from the epoch containing the last processed
//!   height, using the previous boundary block from marshal's archive.

use crate::{
    application::{Application, StateMachine},
    block::Block,
    types::{Digest, PublicKey, Scheme, NAMESPACE},
};
use commonware_actor::Feedback;
use commonware_consensus::{
    marshal::{core::Mailbox as MarshalMailbox, standard::Standard, Update},
    simplex::{self, elector::RoundRobin, Floor, ForwardPolicy, SkipBudget, SkipPolicy},
    types::{Epoch, Epocher, FixedEpocher, Height, ViewDelta},
    Automaton, CertifiableAutomaton, Heightable, Relay, Reporter,
};
use commonware_cryptography::{
    certificate::{Provider, Scoped},
    ed25519::PrivateKey,
    sha256::Sha256,
    Digestible, Signer,
};
use commonware_p2p::{utils::mux::Muxer, Blocker, Manager, Receiver, Sender};
use commonware_parallel::Strategy;
use commonware_runtime::{
    buffer::paged::CacheRef, spawn_cell, BufferPooler, Clock, ContextCell, Handle, Metrics,
    Spawner, Storage,
};
use commonware_utils::{channel::mpsc, ordered::Set, Acknowledgement, NZUsize};
use governor::clock::Clock as GClock;
use rand_core::{CryptoRng, Rng};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::{debug, error, info, warn};

/// Certificate scheme per epoch, shared between marshal and the orchestrator.
#[derive(Clone, Default)]
pub struct EpochProvider {
    schemes: Arc<Mutex<BTreeMap<Epoch, Arc<Scheme>>>>,
}

impl EpochProvider {
    pub fn register(&self, epoch: Epoch, scheme: Scheme) {
        if let Ok(mut m) = self.schemes.lock() {
            m.insert(epoch, Arc::new(scheme));
        }
    }

    pub fn has(&self, epoch: Epoch) -> bool {
        self.schemes
            .lock()
            .map(|m| m.contains_key(&epoch))
            .unwrap_or(false)
    }
}

impl Provider for EpochProvider {
    type Scope = Epoch;
    type Scheme = Scheme;

    fn scoped(&self, scope: Epoch) -> Option<Scoped<Scheme>> {
        self.schemes
            .lock()
            .ok()?
            .get(&scope)
            .cloned()
            .map(Scoped::scheme)
    }

    fn scheme(&self, scope: Epoch) -> Option<Arc<Scheme>> {
        self.schemes.lock().ok()?.get(&scope).cloned()
    }
}

/// Builds the ed25519 scheme for a validator set: a signer when we are a
/// member, a verifier otherwise.
pub fn scheme_for(participants: &Set<PublicKey>, signer: &PrivateKey) -> Scheme {
    match Scheme::signer(NAMESPACE, participants.clone(), signer.clone()) {
        Some(s) => s,
        None => Scheme::verifier(NAMESPACE, participants.clone()),
    }
}

/// Consensus timings shared by every epoch engine.
#[derive(Clone)]
pub struct Timings {
    pub leader_timeout: Duration,
    pub certification_timeout: Duration,
    pub nullify_retry: Duration,
    pub fetch_timeout: Duration,
    pub view_retention: ViewDelta,
    pub skip_timeout: Duration,
    pub mailbox_size: usize,
    pub replay_buffer: std::num::NonZeroUsize,
    pub write_buffer: std::num::NonZeroUsize,
}

/// Timeouts for an epoch whose blocks are paced by `pace` (the larger of
/// the busy and idle intervals): every vote-to-nullify timeout stretches by
/// the same amount, so a leader that waits out the interval is never
/// nullified, and Commonware's ordering invariants (certification >
/// leader, skip > certification and skip > retry) hold as they did unpaced.
pub fn paced_timings(base: &Timings, pace: Duration) -> Timings {
    Timings {
        leader_timeout: base.leader_timeout + pace,
        certification_timeout: base.certification_timeout + pace,
        skip_timeout: base.skip_timeout + pace,
        ..base.clone()
    }
}

/// The message a boundary block turns into.
enum Msg {
    Finalized {
        block: Arc<Block>,
        ack: commonware_utils::acknowledgement::Exact,
    },
}

/// Marshal-side reporter feeding the orchestrator.
#[derive(Clone)]
pub struct EpochReporter {
    tx: mpsc::Sender<Msg>,
}

impl Reporter for EpochReporter {
    type Activity = Update<Block>;

    fn report(&mut self, update: Self::Activity) -> Feedback {
        if let Update::Block(block, ack) = update {
            // Never block marshal: the actor acknowledges after handling.
            if self.tx.try_send(Msg::Finalized { block, ack }).is_err() {
                warn!("epoch orchestrator mailbox full or closed");
            }
        }
        Feedback::Ok
    }
}

struct Active {
    epoch: Epoch,
    handle: Handle<()>,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Runs one simplex engine per epoch. Generic over the marshaled
/// application `A` (Automaton + Relay for the engine).
#[allow(clippy::type_complexity)]
pub struct Orchestrator<E, B, Mg, S, M, A>
where
    E: BufferPooler + Clock + GClock + Rng + CryptoRng + Spawner + Storage + Metrics,
    B: Blocker<PublicKey = PublicKey>,
    Mg: Manager<PublicKey = PublicKey>,
    S: Strategy,
    M: StateMachine,
    A: Automaton<Digest = Digest, Context = crate::types::Context>
        + CertifiableAutomaton
        + Relay<Digest = Digest, PublicKey = PublicKey, Plan = simplex::Plan<PublicKey>>,
{
    context: ContextCell<E>,
    rx: mpsc::Receiver<Msg>,
    blocker: B,
    manager: Mg,
    strategy: S,
    provider: EpochProvider,
    marshal: MarshalMailbox<Scheme, Standard<Block>>,
    automaton: A,
    app: Application<M>,
    signer: PrivateKey,
    genesis_participants: Set<PublicKey>,
    genesis_digest: Digest,
    epocher: FixedEpocher,
    timings: Timings,
    partition_prefix: String,
    page_cache: CacheRef,
}

impl<E, B, Mg, S, M, A> Orchestrator<E, B, Mg, S, M, A>
where
    E: BufferPooler + Clock + GClock + Rng + CryptoRng + Spawner + Storage + Metrics,
    B: Blocker<PublicKey = PublicKey>,
    Mg: Manager<PublicKey = PublicKey>,
    S: Strategy,
    M: StateMachine,
    A: Automaton<Digest = Digest, Context = crate::types::Context>
        + CertifiableAutomaton
        + Relay<Digest = Digest, PublicKey = PublicKey, Plan = simplex::Plan<PublicKey>>,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: E,
        blocker: B,
        manager: Mg,
        strategy: S,
        provider: EpochProvider,
        marshal: MarshalMailbox<Scheme, Standard<Block>>,
        automaton: A,
        app: Application<M>,
        signer: PrivateKey,
        genesis_participants: Set<PublicKey>,
        genesis_digest: Digest,
        epocher: FixedEpocher,
        timings: Timings,
        partition_prefix: String,
        page_cache: CacheRef,
    ) -> (Self, EpochReporter) {
        let (tx, rx) = mpsc::channel(1024);
        (
            Self {
                context: ContextCell::new(context),
                rx,
                blocker,
                manager,
                strategy,
                provider,
                marshal,
                automaton,
                app,
                signer,
                genesis_participants,
                genesis_digest,
                epocher,
                timings,
                partition_prefix,
                page_cache,
            },
            EpochReporter { tx },
        )
    }

    /// Validators of `epoch`: the state machine's answer, else genesis.
    fn participants(&self, epoch: Epoch) -> Set<PublicKey> {
        let from_state = self.app.with(|m| m.validators(epoch.get()));
        match from_state {
            Some(keys) if !keys.is_empty() => {
                let mut pks = Vec::new();
                for k in keys {
                    if let Ok(pk) = PublicKey::try_from(k.as_slice()) {
                        pks.push(pk);
                    }
                }
                if pks.is_empty() {
                    self.genesis_participants.clone()
                } else {
                    Set::from_iter_dedup(pks)
                }
            }
            _ => self.genesis_participants.clone(),
        }
    }

    pub fn start(
        mut self,
        votes: (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
        certificates: (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
        resolver: (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
    ) -> Handle<()> {
        spawn_cell!(self.context, self.run(votes, certificates, resolver))
    }

    async fn run(
        mut self,
        (vs, vr): (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
        (cs, cr): (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
        (rs, rr): (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
    ) {
        let size = self.timings.mailbox_size;
        let (mux, mut votes) = Muxer::new(self.context.child("vote_mux"), vs, vr, size);
        mux.start();
        let (mux, mut certificates) =
            Muxer::new(self.context.child("certificate_mux"), cs, cr, size);
        mux.start();
        let (mux, mut resolvers) = Muxer::new(self.context.child("resolver_mux"), rs, rr, size);
        mux.start();

        // Resume from the epoch containing the last applied height.
        let (applied, _) = self.app.tip();
        let start_epoch = self
            .epocher
            .containing(applied)
            .map(|info| info.epoch())
            .unwrap_or_else(Epoch::zero);
        let floor = if start_epoch.is_zero() {
            Floor::Genesis(self.genesis_digest)
        } else {
            let boundary = start_epoch
                .previous()
                .and_then(|prev| self.epocher.last(prev));
            match boundary {
                Some(h) => match self.marshal.get_block(h).await {
                    Some(block) => Floor::Genesis(block.digest()),
                    None => {
                        error!(%h, "boundary block missing; cannot resume epoch");
                        return;
                    }
                },
                None => Floor::Genesis(self.genesis_digest),
            }
        };
        let mut active = match self
            .enter(
                start_epoch,
                floor,
                &mut votes,
                &mut certificates,
                &mut resolvers,
            )
            .await
        {
            Some(a) => a,
            None => return,
        };

        loop {
            let Some(msg) = self.rx.recv().await else {
                debug!("epoch orchestrator mailbox closed");
                return;
            };
            match msg {
                Msg::Finalized { block, ack } => {
                    let height = block.height();
                    if self.epocher.last(active.epoch) == Some(height) {
                        let next = active.epoch.next();
                        let floor = Floor::Genesis(block.digest());
                        match self
                            .enter(next, floor, &mut votes, &mut certificates, &mut resolvers)
                            .await
                        {
                            Some(a) => {
                                info!(epoch = %next, %height, "epoch rotated");
                                active = a;
                            }
                            None => {
                                ack.acknowledge();
                                return;
                            }
                        }
                    }
                    ack.acknowledge();
                }
            }
        }
    }

    async fn enter<VS, VR, CS, CR, RS, RR>(
        &mut self,
        epoch: Epoch,
        floor: Floor<Scheme, Digest>,
        votes: &mut commonware_p2p::utils::mux::MuxHandle<VS, VR>,
        certificates: &mut commonware_p2p::utils::mux::MuxHandle<CS, CR>,
        resolvers: &mut commonware_p2p::utils::mux::MuxHandle<RS, RR>,
    ) -> Option<Active>
    where
        VS: Sender<PublicKey = PublicKey>,
        VR: Receiver<PublicKey = PublicKey>,
        CS: Sender<PublicKey = PublicKey>,
        CR: Receiver<PublicKey = PublicKey>,
        RS: Sender<PublicKey = PublicKey>,
        RR: Receiver<PublicKey = PublicKey>,
    {
        let participants = self.participants(epoch);
        let scheme = scheme_for(&participants, &self.signer);
        self.provider.register(epoch, scheme.clone());
        let _ = self.manager.track(epoch.get(), participants.clone());
        let is_member = participants.iter().any(|p| *p == self.signer.public_key());
        // Block pacing from state (settable by governance / the param admin):
        // the leader and certification timeouts stretch by the idle interval
        // so a paced leader is never nullified, and the proposer's own cap
        // stays a margin below the leader timeout. Read at the epoch
        // boundary: every validator derives the same timeouts.
        let (busy_ms, idle_ms) = self.app.with(|m| m.block_intervals_ms());
        let paced = paced_timings(&self.timings, Duration::from_millis(idle_ms.max(busy_ms)));
        let leader_timeout = paced.leader_timeout;
        let certification_timeout = paced.certification_timeout;
        self.app
            .set_max_propose_delay(leader_timeout.saturating_sub(self.timings.leader_timeout / 2));
        info!(%epoch, validators = participants.len(), member = is_member, busy_ms, idle_ms, leader_timeout_ms = leader_timeout.as_millis() as u64, "entering epoch");

        let context = self
            .context
            .child("consensus")
            .with_attribute("epoch", epoch);
        let engine = simplex::Engine::new(
            context,
            simplex::Config {
                scheme,
                elector: RoundRobin::<Sha256>::default(),
                blocker: self.blocker.clone(),
                automaton: self.automaton.clone(),
                relay: self.automaton.clone(),
                reporter: self.marshal.clone(),
                track_historical_votes: false,
                strategy: self.strategy.clone(),
                partition: format!("{}-consensus-{}", self.partition_prefix, epoch),
                mailbox_size: NZUsize!(self.timings.mailbox_size),
                epoch,
                floor,
                replay_buffer: self.timings.replay_buffer,
                write_buffer: self.timings.write_buffer,
                page_cache: self.page_cache.clone(),
                leader_timeout,
                certification_timeout,
                timeout_retry: self.timings.nullify_retry,
                view_retention: self.timings.view_retention,
                skip: SkipPolicy::Enabled {
                    timeout: paced.skip_timeout,
                    budget: SkipBudget::Participants,
                },
                fetch_timeout: self.timings.fetch_timeout,
                forward: ForwardPolicy::Disabled,
            },
        );
        let Ok(v) = votes.register(epoch.get()).await else {
            return None;
        };
        let Ok(c) = certificates.register(epoch.get()).await else {
            return None;
        };
        let Ok(r) = resolvers.register(epoch.get()).await else {
            return None;
        };
        let handle = engine.start(v, c, r);
        Some(Active { epoch, handle })
    }
}

/// Height helper for tests and RPC: the epoch a height belongs to.
pub fn epoch_of(epocher: &FixedEpocher, height: u64) -> Option<Epoch> {
    epocher.containing(Height::new(height)).map(|i| i.epoch())
}
