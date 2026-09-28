//! The Keel node: p2p + consensus + VM + RPC.
//!
//! ```sh
//! keel-node --me 0@3000 --participants 0,1,2,3 --devnet --storage-dir /tmp/keel/0
//! keel-node --me 1@3001 --participants 0,1,2,3 --devnet --bootstrappers 0@127.0.0.1:3000 --storage-dir /tmp/keel/1
//! ```
//! RPC listens on `--rpc-port` (default p2p port + 2000), metrics on +1000.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod archive;
mod gossip;
mod machine;
mod sync;

use clap::Parser;
use commonware_consensus::marshal::resolver::p2p as resolver;
use commonware_cryptography::Signer as _;
use commonware_p2p::{
    authenticated::{self, discovery},
    Manager as _,
};
use commonware_parallel::Sequential;
use commonware_runtime::{tokio, Clock as _, Quota, Runner as _, Spawner as _, Supervisor as _};
use commonware_utils::{
    ordered::{Quorum as _, Set},
    union, NZUsize, NZU32,
};
use keel_actions::CHAIN_ID_DEVNET;
use keel_consensus::{
    application::Application,
    engine::{self, Engine},
    types::{PrivateKey, PublicKey, NAMESPACE},
};
use keel_crypto::Keypair;
use keel_rpc::NodeApi as _;
use keel_vm::{genesis::GenesisValidator, Genesis, State};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tracing::{info, Level};

const VOTES: u64 = 0;
const CERTIFICATES: u64 = 1;
const RESOLVER: u64 = 2;
const BROADCAST: u64 = 3;
const BACKFILL: u64 = 4;
const ACTIONS: u64 = 5;
const MAX_MESSAGE_SIZE: u32 = 8 * 1024 * 1024;

#[derive(Parser, Debug)]
#[command(name = "keel-node", about = "Keelchain validator")]
struct Args {
    /// `<seed>@<port>`: this validator's key seed and listen port.
    #[arg(long)]
    me: String,
    /// Every validator's consensus key, including this one: a key seed
    /// (devnet) or a 64-hex ed25519 public key. Omit with `--genesis` to
    /// take the set from the genesis file's validators.
    #[arg(long, value_delimiter = ',')]
    participants: Vec<String>,
    /// `<seed|pubkey>@<ip:port>` of peers to dial first.
    #[arg(long, value_delimiter = ',')]
    bootstrappers: Vec<String>,
    /// `validator` (this key must be in the validator set) or `follower`
    /// (verify and serve only, never vote). Default: validator when the key
    /// is in the set, follower otherwise.
    #[arg(long)]
    role: Option<String>,
    /// RPC base URL of a running node to fetch the newest snapshot from
    /// when this node's storage is empty (state sync), e.g.
    /// https://testnet.keelchain.com/rpc.
    #[arg(long)]
    sync_from: Option<String>,
    /// A second node's RPC whose block record must confirm the snapshot's
    /// tip hash before it is installed.
    #[arg(long)]
    sync_verify: Option<String>,
    /// Keys (seed or 64-hex) of follower nodes to keep connected in every
    /// epoch, on top of what the chain state knows (bonded validators and
    /// observers). Validators list their RPC followers here.
    #[arg(long, value_delimiter = ',')]
    extra_peers: Vec<String>,
    /// Public `<ip:port>` other validators reach this node at. When set the
    /// p2p socket listens on 0.0.0.0:<port> with the production peer
    /// config; unset = loopback-only devnet behaviour.
    #[arg(long)]
    advertise: Option<String>,
    /// Upper bound of the peer set (validators plus followers). Defaults to
    /// today's peers plus eight; each slot costs memory up front.
    #[arg(long)]
    max_peers: Option<usize>,
    /// Address the HTTP/WS API binds to.
    #[arg(long, default_value = "127.0.0.1")]
    rpc_listen: IpAddr,
    /// Chain id for a `--devnet` genesis (default: the devnet id).
    #[arg(long)]
    chain_id: Option<u32>,
    /// Print the genesis (JSON) that this node would start from and exit.
    #[arg(long)]
    print_genesis: bool,
    /// Print this node's public identity for `keel genesis-build` and exit:
    /// the consensus key and the account address derived from `--me`.
    #[arg(long)]
    print_identity: bool,
    #[arg(long)]
    storage_dir: String,
    #[arg(long, default_value = "info")]
    log_level: String,
    /// Genesis file (serde JSON of keel_vm::Genesis).
    #[arg(long)]
    genesis: Option<PathBuf>,
    /// Build a devnet genesis: participants are validators, observers,
    /// arbitrators and attesters, and their account keys are funded.
    #[arg(long)]
    devnet: bool,
    /// Devnet only: idle block interval in milliseconds (the chain default
    /// is 5000). Every node of the devnet must pass the same value.
    #[arg(long)]
    devnet_idle_ms: Option<u32>,
    /// HTTP/WS API port (default: p2p port + 2000).
    #[arg(long)]
    rpc_port: Option<u16>,
    /// External network the vault deposit addresses are encoded for:
    /// mainnet | testnet | signet | regtest (devnet default: regtest).
    #[arg(long)]
    external_network: Option<String>,
    /// Write a state snapshot every N blocks.
    #[arg(long, default_value_t = 200)]
    snapshot_interval: u64,
    /// Keep only this many blocks of journal below the newest snapshot and
    /// delete older segments (0 = keep the full history). Validators keep
    /// everything; a follower can prune.
    #[arg(long, default_value_t = 0)]
    retain_blocks: u64,
    /// Consensus epoch length in blocks (default: the staking module's
    /// `epoch_length_blocks`, so validator-set changes line up).
    #[arg(long)]
    blocks_per_epoch: Option<u64>,
}

/// Devnet genesis: one account key and one consensus key per seed.
fn devnet_genesis(seeds: &[u64], chain_id: u32, idle_ms: Option<u32>) -> Genesis {
    let funded: Vec<_> = seeds
        .iter()
        .map(|s| Keypair::from_seed(*s).address())
        .collect();
    let params = keel_vm::Params::default();
    let validators = seeds
        .iter()
        .map(|s| {
            let pk = PrivateKey::from_seed(*s).public_key();
            let mut key = [0u8; 32];
            key.copy_from_slice(pk.as_ref());
            GenesisValidator {
                address: Keypair::from_seed(*s).address(),
                consensus_key: key,
                bond: params.min_validator_bond,
            }
        })
        .collect();
    let mut g = Genesis::devnet(chain_id, &funded, validators);
    // The first seed doubles as the super admin so `SetParam` (the
    // backoffice "Chain parameters" card) works on a devnet.
    g.param_admin = seeds.first().map(|s| Keypair::from_seed(*s).address());
    if let Some(ms) = idle_ms {
        g.params.idle_block_interval_ms = ms;
        g.params.min_block_interval_ms = g.params.min_block_interval_ms.min(ms);
    }
    g
}

/// A participant is a devnet key seed or a 64-hex ed25519 public key.
fn parse_key(s: &str) -> anyhow::Result<PublicKey> {
    let s = s.trim();
    if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes = hex::decode(s)?;
        return PublicKey::try_from(bytes.as_slice())
            .map_err(|_| anyhow::anyhow!("invalid public key {s}"));
    }
    let seed: u64 = s
        .parse()
        .map_err(|_| anyhow::anyhow!("participant {s:?} is neither a seed nor a 64-hex key"))?;
    Ok(PrivateKey::from_seed(seed).public_key())
}

fn genesis_of(args: &Args) -> anyhow::Result<Genesis> {
    match (&args.genesis, args.devnet) {
        (Some(path), _) => {
            let text = std::fs::read_to_string(path)?;
            Ok(serde_json::from_str::<Genesis>(&text)?)
        }
        (None, true) => {
            let seeds = args
                .participants
                .iter()
                .map(|s| {
                    s.parse::<u64>().map_err(|_| {
                        anyhow::anyhow!("--devnet participants must be key seeds, got {s:?}")
                    })
                })
                .collect::<anyhow::Result<Vec<u64>>>()?;
            anyhow::ensure!(!seeds.is_empty(), "--devnet needs --participants");
            Ok(devnet_genesis(
                &seeds,
                args.chain_id.unwrap_or(CHAIN_ID_DEVNET),
                args.devnet_idle_ms,
            ))
        }
        (None, false) => anyhow::bail!("pass --genesis <file> or --devnet"),
    }
}

fn load_genesis(genesis: Genesis, storage: &std::path::Path) -> anyhow::Result<State> {
    std::fs::create_dir_all(storage)?;
    std::fs::write(
        storage.join("genesis.json"),
        serde_json::to_string_pretty(&genesis)?,
    )?;
    Ok(genesis.build())
}

fn main() {
    let args = Args::parse();
    let (seed, port) = args.me.split_once('@').expect("--me must be <seed>@<port>");
    let signer = PrivateKey::from_seed(seed.parse().expect("seed"));
    let port: u16 = port.parse().expect("port");
    let me = signer.public_key();
    let rpc_port = args.rpc_port.unwrap_or(port + 2000);
    let rpc_listen = args.rpc_listen;
    if args.print_identity {
        let seed: u64 = seed.parse().expect("seed");
        println!(
            "{}",
            serde_json::json!({
                "consensus_key": hex::encode(me.as_ref()),
                "address": Keypair::from_seed(seed).address().to_hex(),
            })
        );
        return;
    }

    let genesis_doc = genesis_of(&args).expect("genesis");
    if args.print_genesis {
        println!(
            "{}",
            serde_json::to_string_pretty(&genesis_doc).expect("genesis json")
        );
        return;
    }
    let participants: Vec<PublicKey> = if args.participants.is_empty() {
        // The validator set is what the genesis says it is.
        genesis_doc
            .validators
            .iter()
            .map(|v| {
                PublicKey::try_from(v.consensus_key.as_slice()).expect("genesis consensus key")
            })
            .collect()
    } else {
        args.participants
            .iter()
            .map(|s| parse_key(s).expect("participant"))
            .collect()
    };
    let validators: Set<PublicKey> = Set::from_iter_dedup(participants);
    let in_set = validators.index(&me).is_some();
    let role = match args.role.as_deref() {
        Some("validator") => {
            assert!(
                in_set,
                "--role validator needs --me in the validator set (--participants or the genesis validators)"
            );
            "validator"
        }
        Some("follower") => "follower",
        Some(other) => panic!("--role must be validator or follower, not {other}"),
        None => {
            if in_set {
                "validator"
            } else {
                "follower"
            }
        }
    };
    let extra_peers: Set<PublicKey> = Set::from_iter_dedup(
        args.extra_peers
            .iter()
            .map(|s| parse_key(s).expect("extra peer key"))
            .collect::<Vec<_>>(),
    );
    // Connections are accepted only from tracked peers, so the tracked set
    // is the validators plus every follower, ourselves included.
    let tracked: Set<PublicKey> = Set::from_iter_dedup(
        validators
            .iter()
            .cloned()
            .chain(extra_peers.iter().cloned())
            .chain(std::iter::once(me.clone()))
            .collect::<Vec<_>>(),
    );
    // Peer sets grow when validators bond after genesis and the p2p layer
    // asserts on a set larger than this limit, so leave headroom: eight
    // slots beyond today's peers, `--max-peers` to override. The p2p layer
    // reserves buffers per slot (about 18 MB each), so the limit is memory.
    let natural = authenticated::peer_set_limit(&tracked, &me).get();
    let limit = args
        .max_peers
        .unwrap_or_else(|| (tracked.len() + 8).clamp(natural, 64))
        .max(natural);
    let max_peers_per_set = std::num::NonZeroUsize::new(limit).expect("peer limit");

    let bootstrappers = args
        .bootstrappers
        .iter()
        .map(|b| {
            let (s, addr) = b
                .split_once('@')
                .expect("--bootstrappers entries are <seed|pubkey>@<ip:port>");
            let key = parse_key(s).expect("bootstrapper key");
            (
                key,
                SocketAddr::from_str(addr).expect("socket address").into(),
            )
        })
        .collect();

    let storage_dir = PathBuf::from(&args.storage_dir);
    if let Some(from) = args.sync_from.as_deref() {
        sync::bootstrap(&storage_dir.join("vm"), from, args.sync_verify.as_deref())
            .expect("state sync");
    }
    let genesis = load_genesis(genesis_doc, &storage_dir).expect("genesis");
    let (machine, shared) = machine::VmMachine::open(
        &storage_dir.join("vm"),
        genesis,
        args.snapshot_interval,
        args.retain_blocks,
    )
    .expect("open vm storage");
    let genesis_hash = hex::encode(shared.state.lock().expect("state").last_hash);
    let blocks_per_epoch = args.blocks_per_epoch.unwrap_or_else(|| {
        shared
            .state
            .lock()
            .expect("state")
            .params
            .epoch_length_blocks
    });
    let blocks_per_epoch =
        std::num::NonZeroU64::new(blocks_per_epoch).expect("blocks-per-epoch must be > 0");

    let runtime_cfg = tokio::Config::new().with_storage_directory(args.storage_dir.clone());
    let executor = tokio::Runner::new(runtime_cfg);
    let p2p_cfg = match &args.advertise {
        // A real network: listen on every interface, tell peers the public
        // address, production dial/handshake limits (no private IPs).
        Some(public) => discovery::Config::recommended(
            signer.clone(),
            &union(NAMESPACE, b"_P2P"),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
            SocketAddr::from_str(public).expect("--advertise must be <ip:port>"),
            bootstrappers,
            max_peers_per_set,
            MAX_MESSAGE_SIZE,
        ),
        None => discovery::Config::local(
            signer.clone(),
            &union(NAMESPACE, b"_P2P"),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            bootstrappers,
            max_peers_per_set,
            MAX_MESSAGE_SIZE,
        ),
    };
    let level = Level::from_str(&args.log_level).expect("log level");
    let validator_names: Vec<String> = validators.iter().map(|v| hex::encode(v.as_ref())).collect();

    executor.start(|context| async move {
        tokio::telemetry::init(
            context.child("telemetry"),
            tokio::telemetry::Logs { level, json: false },
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port + 1000)),
            None,
        );
        info!(?me, port, rpc_port, role, validators = validators.len(), followers = extra_peers.len(), state = %genesis_hash, "starting");

        let (mut network, mut oracle) = discovery::Network::new(context.child("network"), p2p_cfg);
        oracle.track(0, tracked.clone());
        let rate = Quota::per_second(NZU32!(1024));
        let votes = network.register(VOTES, rate);
        let certificates = network.register(CERTIFICATES, rate);
        let res = network.register(RESOLVER, rate);
        let broadcast = network.register(BROADCAST, rate);
        let backfill = network.register(BACKFILL, rate);
        let (action_tx, action_rx) = network.register(ACTIONS, Quota::per_second(NZU32!(4096)));
        let p2p = network.start();

        let app = Application::new(machine);
        let mut cfg = engine::devnet_timings(
            oracle.clone(),
            oracle.clone(),
            oracle.clone(),
            signer,
            validators,
            "keel".to_string(),
            Sequential,
        );
        cfg.blocks_per_epoch = blocks_per_epoch;
        cfg.extra_peers = extra_peers;
        let engine = Engine::new(context.child("engine"), app.clone(), cfg).await;
        let backfill = resolver::init(
            context.child("backfill"),
            resolver::Config {
                public_key: me.clone(),
                peer_provider: oracle.clone(),
                blocker: oracle,
                mailbox_size: NZUsize!(1024),
                timeout: Duration::from_secs(2),
                fetch_retry_timeout: Duration::from_millis(100),
                priority_requests: false,
                priority_responses: false,
            },
            backfill,
        );
        let engine = engine.start(votes, certificates, res, broadcast, backfill);

        // Gossip + RPC.
        let (outbound, outbound_rx) = ::tokio::sync::mpsc::unbounded_channel();
        let external_network = match args.external_network.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("mainnet") => keel_chains::Network::Mainnet,
            Some("testnet") => keel_chains::Network::Testnet,
            Some("signet") => keel_chains::Network::Signet,
            Some("regtest") => keel_chains::Network::Regtest,
            Some(other) => panic!("unknown --external-network {other}"),
            None if args.devnet => keel_chains::Network::Regtest,
            None => keel_chains::Network::Mainnet,
        };
        let node = Arc::new(gossip::Node { shared, outbound, validators: validator_names, external_network });
        let send = context.child("gossip_send").spawn(move |_| gossip::send_loop(action_tx, outbound_rx));
        let recv_node = node.clone();
        let recv = context.child("gossip_recv").spawn(move |_| gossip::recv_loop(recv_node, action_rx));
        let rpc_node: Arc<dyn keel_rpc::NodeApi> = node.clone();
        let rpc = context.child("rpc").spawn(move |_| async move {
            if let Err(e) = keel_rpc::serve(rpc_node, SocketAddr::new(rpc_listen, rpc_port)).await {
                tracing::error!(?e, "rpc stopped");
            }
        });

        let ticker = context.child("ticker");
        let ticker = async move {
            loop {
                ticker.sleep(Duration::from_secs(2)).await;
                let (height, state) = app.tip();
                info!(%height, ?state, mempool = node.mempool_len(), "tip");
            }
        };
        futures::pin_mut!(ticker);
        let stack = futures::future::try_join_all(vec![engine, p2p, send, recv, rpc]);
        futures::pin_mut!(stack);
        match futures::future::select(ticker, stack).await {
            futures::future::Either::Left(_) => {}
            futures::future::Either::Right((result, _)) => {
                if let Err(e) = result {
                    tracing::error!(?e, "node stopped");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn participants_accept_seeds_and_hex_keys() {
        let from_seed = parse_key("7").unwrap();
        let hex_key = hex::encode(from_seed.as_ref());
        assert_eq!(parse_key(&hex_key).unwrap(), from_seed);
        assert_eq!(parse_key(" 7 ").unwrap(), from_seed);
        assert!(parse_key("zz").is_err());
        assert!(parse_key(&"a".repeat(63)).is_err()); // neither a seed nor a full key
    }

    #[test]
    fn devnet_genesis_carries_the_chain_id() {
        let g = devnet_genesis(&[1, 2], 42, None);
        assert_eq!(g.chain_id, 42);
        assert_eq!(g.validators.len(), 2);
        assert_eq!(g.param_admin, Some(Keypair::from_seed(1).address()));
    }
}
