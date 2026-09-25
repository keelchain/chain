//! Constructs and starts the consensus stack for one validator:
//! buffered broadcast (full blocks) → marshal (ordered finalized delivery,
//! backfill, archives) → epoch orchestrator (one simplex engine per epoch).
//! Adapted from the Alto reference chain, with plain ed25519 certificates,
//! round-robin leader election and validator-set rotation.

use crate::{
    application::{Application, StateMachine},
    block::Block,
    epochs::{scheme_for, EpochProvider, EpochReporter, Orchestrator, Timings},
    types::{Digest, PublicKey, Scheme, EPOCH, EPOCH_LENGTH},
};
use commonware_broadcast::buffered;
use commonware_consensus::{
    marshal::{
        self,
        core::Actor as MarshalActor,
        resolver::handler,
        standard::{Deferred, Standard},
    },
    types::{FixedEpocher, ViewDelta},
    Reporters,
};
use commonware_cryptography::{
    certificate::Verifier as _, ed25519::PrivateKey, Digestible, Signer as _,
};
use commonware_p2p::{Blocker, Manager, Provider, Receiver, Sender};
use commonware_parallel::Strategy;
use commonware_resolver::TargetedResolver;
use commonware_runtime::{
    buffer::paged::CacheRef, spawn_cell, BufferPooler, Clock, ContextCell, Handle, Metrics,
    Spawner, Storage,
};
use commonware_storage::archive::immutable;
use commonware_utils::{ordered::Set, NZUsize, NZU16, NZU64};
use futures::future::try_join_all;
use governor::clock::Clock as GClock;
use rand_core::{CryptoRng, Rng};
use std::{
    num::{NonZero, NonZeroU64},
    time::Duration,
};
use tracing::{error, warn};

const SYNCER_ACTIVITY_TIMEOUT_MULTIPLIER: u64 = 10;
const PRUNABLE_ITEMS_PER_SECTION: NonZero<u64> = NZU64!(4_096);
const IMMUTABLE_ITEMS_PER_SECTION: NonZero<u64> = NZU64!(262_144);
const FREEZER_TABLE_RESIZE_FREQUENCY: u8 = 4;
const FREEZER_TABLE_RESIZE_CHUNK_SIZE: u32 = 2u32.pow(16);
const FREEZER_JOURNAL_TARGET_SIZE: u64 = 1024 * 1024 * 1024;
const FREEZER_JOURNAL_COMPRESSION: Option<u8> = Some(3);
const REPLAY_BUFFER: NonZero<usize> = NZUsize!(8 * 1024 * 1024);
const WRITE_BUFFER: NonZero<usize> = NZUsize!(1024 * 1024);
const PAGE_CACHE_PAGE_SIZE: NonZero<u16> = NZU16!(4_084);
const PAGE_CACHE_CAPACITY: NonZero<usize> = NZUsize!(8_192);
const MAX_REPAIR: NonZero<usize> = NZUsize!(20);
const MAX_PENDING_ACKS: NonZero<usize> = NZUsize!(16);

/// Everything a validator needs besides network channels.
pub struct Config<B, P, Mg, S>
where
    B: Blocker<PublicKey = PublicKey>,
    P: Provider<PublicKey = PublicKey>,
    Mg: Manager<PublicKey = PublicKey>,
    S: Strategy,
{
    pub blocker: B,
    pub provider: P,
    pub manager: Mg,
    pub partition_prefix: String,
    pub freezer_table_initial_size: u32,
    pub signer: PrivateKey,
    /// Genesis validator set; later epochs come from the state machine.
    pub participants: Set<PublicKey>,
    pub blocks_per_epoch: NonZeroU64,
    pub mailbox_size: usize,
    pub deque_size: usize,
    pub leader_timeout: Duration,
    pub certification_timeout: Duration,
    pub nullify_retry: Duration,
    pub fetch_timeout: Duration,
    pub view_retention: ViewDelta,
    pub skip_timeout: Duration,
    pub strategy: S,
}

type Marshaled<E, M> = Deferred<E, Scheme, Application<M>, Block, FixedEpocher>;

/// The per-validator engine.
#[allow(clippy::type_complexity)]
pub struct Engine<E, B, P, Mg, S, M>
where
    E: BufferPooler + Clock + GClock + Rng + CryptoRng + Spawner + Storage + Metrics,
    B: Blocker<PublicKey = PublicKey>,
    P: Provider<PublicKey = PublicKey>,
    Mg: Manager<PublicKey = PublicKey>,
    S: Strategy,
    M: StateMachine,
{
    context: ContextCell<E>,
    buffer: buffered::Engine<E, PublicKey, Block, P>,
    buffer_mailbox: buffered::Mailbox<PublicKey, Block>,
    marshal: MarshalActor<
        E,
        Standard<Block>,
        EpochProvider,
        immutable::Archive<E, Digest, crate::types::Finalization>,
        immutable::Archive<E, Digest, Block>,
        FixedEpocher,
        S,
    >,
    marshaled: Marshaled<E, M>,
    orchestrator: Orchestrator<E, B, Mg, S, M, Marshaled<E, M>>,
    epoch_reporter: EpochReporter,
    /// Exposed for tests and RPC: epochs whose scheme is registered.
    pub provider: EpochProvider,
}

impl<E, B, P, Mg, S, M> Engine<E, B, P, Mg, S, M>
where
    E: BufferPooler + Clock + GClock + Rng + CryptoRng + Spawner + Storage + Metrics,
    B: Blocker<PublicKey = PublicKey>,
    P: Provider<PublicKey = PublicKey>,
    Mg: Manager<PublicKey = PublicKey>,
    S: Strategy,
    M: StateMachine,
{
    pub async fn new(context: E, app: Application<M>, cfg: Config<B, P, Mg, S>) -> Self {
        let me = cfg.signer.public_key();
        let (buffer, buffer_mailbox) = buffered::Engine::new(
            context.child("buffer"),
            buffered::Config {
                public_key: me,
                mailbox_size: NZUsize!(cfg.mailbox_size),
                deque_size: cfg.deque_size,
                priority: true,
                codec_config: (),
                peer_provider: cfg.provider,
            },
        );

        let page_cache = CacheRef::from_pooler(&context, PAGE_CACHE_PAGE_SIZE, PAGE_CACHE_CAPACITY);

        let finalizations_by_height = immutable::Archive::init(
            context.child("finalizations_by_height"),
            immutable::Config {
                metadata_partition: format!("{}-fin-metadata", cfg.partition_prefix),
                freezer_table_partition: format!("{}-fin-freezer-table", cfg.partition_prefix),
                freezer_table_initial_size: cfg.freezer_table_initial_size,
                freezer_table_resize_frequency: FREEZER_TABLE_RESIZE_FREQUENCY,
                freezer_table_resize_chunk_size: FREEZER_TABLE_RESIZE_CHUNK_SIZE,
                freezer_key_partition: format!("{}-fin-freezer-key", cfg.partition_prefix),
                freezer_key_page_cache: page_cache.clone(),
                freezer_key_write_buffer: WRITE_BUFFER,
                freezer_value_partition: format!("{}-fin-freezer-value", cfg.partition_prefix),
                freezer_value_write_buffer: WRITE_BUFFER,
                freezer_value_target_size: FREEZER_JOURNAL_TARGET_SIZE,
                freezer_value_compression: FREEZER_JOURNAL_COMPRESSION,
                ordinal_partition: format!("{}-fin-ordinal", cfg.partition_prefix),
                ordinal_write_buffer: WRITE_BUFFER,
                items_per_section: IMMUTABLE_ITEMS_PER_SECTION,
                codec_config: Scheme::certificate_codec_config_unbounded(),
                replay_buffer: REPLAY_BUFFER,
            },
        )
        .await
        .expect("failed to initialize finalizations archive");

        let finalized_blocks = immutable::Archive::init(
            context.child("finalized_blocks"),
            immutable::Config {
                metadata_partition: format!("{}-blocks-metadata", cfg.partition_prefix),
                freezer_table_partition: format!("{}-blocks-freezer-table", cfg.partition_prefix),
                freezer_table_initial_size: cfg.freezer_table_initial_size,
                freezer_table_resize_frequency: FREEZER_TABLE_RESIZE_FREQUENCY,
                freezer_table_resize_chunk_size: FREEZER_TABLE_RESIZE_CHUNK_SIZE,
                freezer_key_partition: format!("{}-blocks-freezer-key", cfg.partition_prefix),
                freezer_key_page_cache: page_cache.clone(),
                freezer_key_write_buffer: WRITE_BUFFER,
                freezer_value_partition: format!("{}-blocks-freezer-value", cfg.partition_prefix),
                freezer_value_write_buffer: WRITE_BUFFER,
                freezer_value_target_size: FREEZER_JOURNAL_TARGET_SIZE,
                freezer_value_compression: FREEZER_JOURNAL_COMPRESSION,
                ordinal_partition: format!("{}-blocks-ordinal", cfg.partition_prefix),
                ordinal_write_buffer: WRITE_BUFFER,
                items_per_section: IMMUTABLE_ITEMS_PER_SECTION,
                codec_config: (),
                replay_buffer: REPLAY_BUFFER,
            },
        )
        .await
        .expect("failed to initialize blocks archive");

        // Epoch 0 uses the genesis set; the orchestrator registers the rest.
        let provider = EpochProvider::default();
        provider.register(EPOCH, scheme_for(&cfg.participants, &cfg.signer));
        let epocher = FixedEpocher::new(cfg.blocks_per_epoch);
        let genesis = Application::<M>::genesis();
        let genesis_digest = genesis.digest();
        let (marshal, marshal_mailbox, _floor) = MarshalActor::init(
            context.child("marshal"),
            finalizations_by_height,
            finalized_blocks,
            marshal::Config {
                provider: provider.clone(),
                epocher: epocher.clone(),
                partition_prefix: cfg.partition_prefix.clone(),
                mailbox_size: NZUsize!(cfg.mailbox_size),
                view_retention: ViewDelta::new(
                    cfg.view_retention
                        .get()
                        .saturating_mul(SYNCER_ACTIVITY_TIMEOUT_MULTIPLIER),
                ),
                start: marshal::Start::Genesis(genesis),
                prunable_items_per_section: PRUNABLE_ITEMS_PER_SECTION,
                replay_buffer: REPLAY_BUFFER,
                key_write_buffer: WRITE_BUFFER,
                value_write_buffer: WRITE_BUFFER,
                block_codec_config: (),
                max_repair: MAX_REPAIR,
                max_pending_acks: MAX_PENDING_ACKS,
                page_cache: page_cache.clone(),
                strategy: cfg.strategy.clone(),
            },
        )
        .await;

        let marshaled = Marshaled::new(
            context.child("marshaled"),
            app.clone(),
            marshal_mailbox.clone(),
            epocher.clone(),
        );
        let timings = Timings {
            leader_timeout: cfg.leader_timeout,
            certification_timeout: cfg.certification_timeout,
            nullify_retry: cfg.nullify_retry,
            fetch_timeout: cfg.fetch_timeout,
            view_retention: cfg.view_retention,
            skip_timeout: cfg.skip_timeout,
            mailbox_size: cfg.mailbox_size,
            replay_buffer: REPLAY_BUFFER,
            write_buffer: WRITE_BUFFER,
        };
        let (orchestrator, epoch_reporter) = Orchestrator::new(
            context.child("epochs"),
            cfg.blocker,
            cfg.manager,
            cfg.strategy,
            provider.clone(),
            marshal_mailbox,
            marshaled.clone(),
            app,
            cfg.signer,
            cfg.participants,
            genesis_digest,
            epocher,
            timings,
            cfg.partition_prefix,
            page_cache,
        );

        Self {
            context: ContextCell::new(context),
            buffer,
            buffer_mailbox,
            marshal,
            marshaled,
            orchestrator,
            epoch_reporter,
            provider,
        }
    }

    /// Start every actor. Channels: consensus votes, consensus certificates,
    /// consensus resolver, block broadcast, marshal backfill.
    #[allow(clippy::type_complexity)]
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
        broadcast: (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
        backfill: (
            handler::Receiver<Digest>,
            impl TargetedResolver<
                Key = handler::Key<Digest>,
                Subscriber = handler::Annotation,
                PublicKey = PublicKey,
            >,
        ),
    ) -> Handle<()> {
        spawn_cell!(
            self.context,
            self.run(votes, certificates, resolver, broadcast, backfill)
        )
    }

    #[allow(clippy::type_complexity)]
    async fn run(
        self,
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
        broadcast: (
            impl Sender<PublicKey = PublicKey>,
            impl Receiver<PublicKey = PublicKey>,
        ),
        backfill: (
            handler::Receiver<Digest>,
            impl TargetedResolver<
                Key = handler::Key<Digest>,
                Subscriber = handler::Annotation,
                PublicKey = PublicKey,
            >,
        ),
    ) {
        let buffer = self.buffer.start(broadcast);
        // Application first, orchestrator second: a boundary block is
        // applied before the next validator set is read from it.
        let reporters: Reporters<_, Marshaled<E, M>, EpochReporter> =
            (self.marshaled, self.epoch_reporter).into();
        let marshal = self.marshal.start(reporters, self.buffer_mailbox, backfill);
        let consensus = self.orchestrator.start(votes, certificates, resolver);
        let handles: Vec<Handle<()>> = vec![buffer, marshal, consensus];
        if let Err(e) = try_join_all(handles).await {
            error!(?e, "engine failed");
        } else {
            warn!("engine stopped");
        }
    }
}

/// Sensible devnet timings: ~250 ms views on a LAN, single epoch unless
/// `blocks_per_epoch` is overridden.
#[allow(clippy::too_many_arguments)]
pub fn devnet_timings<B, P, Mg, S>(
    blocker: B,
    provider: P,
    manager: Mg,
    signer: PrivateKey,
    participants: Set<PublicKey>,
    partition_prefix: String,
    strategy: S,
) -> Config<B, P, Mg, S>
where
    B: Blocker<PublicKey = PublicKey>,
    P: Provider<PublicKey = PublicKey>,
    Mg: Manager<PublicKey = PublicKey>,
    S: Strategy,
{
    Config {
        blocker,
        provider,
        manager,
        partition_prefix,
        freezer_table_initial_size: 2u32.pow(16),
        signer,
        participants,
        blocks_per_epoch: EPOCH_LENGTH,
        mailbox_size: 1024,
        deque_size: 10,
        leader_timeout: Duration::from_millis(500),
        certification_timeout: Duration::from_millis(1_000),
        nullify_retry: Duration::from_secs(5),
        fetch_timeout: Duration::from_secs(1),
        view_retention: ViewDelta::new(32),
        skip_timeout: Duration::from_secs(6),
        strategy,
    }
}
