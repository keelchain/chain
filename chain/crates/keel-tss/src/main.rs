//! `keel-tss`: the signer daemon of one observer.
//!
//! ```text
//! keel-tss gen-primes --out primes.json
//! keel-tss keygen --index 0 --n 4 --t 3 --peers h0:7000,h1:7000,h2:7000,h3:7000 \
//!         --eid keel-vault:BTC:epoch:1 --out share.enc [--primes primes.json]
//! keel-tss pubkey --share share.enc
//! keel-tss serve  --share share.enc --listen 127.0.0.1:7000 --peers ... --http 127.0.0.1:7100
//! ```
//!
//! Environment: `KEEL_TSS_PASSPHRASE` (share file), `KEEL_TSS_SECRET` (hex,
//! HMAC key shared by the signer set for the TCP transport).
//!
//! `serve` exposes `POST /sign {"digest": hex32, "path": [u32, ...],
//! "context": {...}}` → `{"r": hex, "s": hex, "v": 0|1}` for the local
//! observer daemon only (bind it to loopback). The receiving party
//! coordinates: it picks the signer subset, sends a `control` announcement
//! (digest, path and context) to every signer and each of them joins the
//! session. With `--policy-rpc <node>` every party, coordinator and joiner,
//! first checks the context against the chain's outbound rows
//! (`keel_chains::policy`): the digest must be the signing hash of a
//! transaction that pays open outbounds of the named batch, from the
//! vault's own keys. Without the flag (devnet) any digest is signed.
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::{bail, Context as _};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use clap::{Parser, Subcommand};
use keel_chains::policy::SignContext;
use keel_tss::{
    ecdsa::{self, EcdsaSignature, KeyShare, KeygenParams, Primes},
    protocol::Mailbox,
    store,
    transport::{TcpTransport, Transport, TransportError, WireMessage},
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

mod policy;

const SECRET_ENV: &str = "KEEL_TSS_SECRET";
const CONTROL_SESSION: &str = "control";

#[derive(Parser)]
#[command(name = "keel-tss", about = "KEEL threshold signer")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate the Paillier safe primes once (slow) and cache them.
    GenPrimes {
        #[arg(long)]
        out: PathBuf,
    },
    /// Run the distributed key generation ceremony.
    Keygen {
        #[arg(long)]
        index: u16,
        #[arg(long)]
        n: u16,
        #[arg(long)]
        t: u16,
        /// Comma-separated host:port of every party, in index order.
        #[arg(long, value_delimiter = ',')]
        peers: Vec<SocketAddr>,
        /// Listen address when it differs from `peers[index]`.
        #[arg(long)]
        listen: Option<SocketAddr>,
        /// Ceremony id every party agrees on.
        #[arg(long)]
        eid: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        primes: Option<PathBuf>,
        #[arg(long, default_value = "1800")]
        timeout_secs: u64,
    },
    /// Print the vault public key and chain code of a share.
    Pubkey {
        #[arg(long)]
        share: PathBuf,
    },
    /// Serve signing requests for the local observer daemon.
    Serve {
        #[arg(long)]
        share: PathBuf,
        #[arg(long, value_delimiter = ',')]
        peers: Vec<SocketAddr>,
        #[arg(long)]
        listen: Option<SocketAddr>,
        #[arg(long, default_value = "127.0.0.1:7100")]
        http: SocketAddr,
        /// Signer subset this party proposes when it coordinates
        /// (defaults to itself plus the lowest other indexes).
        #[arg(long, value_delimiter = ',')]
        signers: Option<Vec<u16>>,
        #[arg(long, default_value = "120")]
        timeout_secs: u64,
        /// Node RPC whose outbound rows every signing request must match
        /// (the signing policy). Unset = sign any digest (devnet only).
        #[arg(long)]
        policy_rpc: Option<String>,
    },
}

fn secret_from_env() -> anyhow::Result<Vec<u8>> {
    let s = std::env::var(SECRET_ENV).with_context(|| format!("{SECRET_ENV} is not set"))?;
    hex::decode(s.trim()).with_context(|| format!("{SECRET_ENV} must be hex"))
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match Cli::parse().cmd {
        Cmd::GenPrimes { out } => {
            let primes = ecdsa::generate_primes();
            std::fs::write(&out, serde_json::to_vec(&primes)?)?;
            println!("wrote {}", out.display());
        }
        Cmd::Keygen {
            index,
            n,
            t,
            peers,
            listen,
            eid,
            out,
            primes,
            timeout_secs,
        } => {
            if peers.len() != n as usize {
                bail!("--peers must list exactly n={n} addresses");
            }
            let passphrase = store::passphrase_from_env()?;
            let secret = secret_from_env()?;
            let primes = match primes {
                Some(p) => Some(
                    serde_json::from_slice::<Primes>(&std::fs::read(&p)?).context("primes file")?,
                ),
                None => None,
            };
            let transport = match listen {
                Some(l) => TcpTransport::bind_on(l, index, peers, secret)?,
                None => TcpTransport::bind(index, peers, secret)?,
            };
            let params = KeygenParams {
                t,
                n,
                my_index: index,
                execution_id: eid.into_bytes(),
                primes,
                timeout: Duration::from_secs(timeout_secs),
            };
            let share = ecdsa::keygen(params, &transport)?;
            store::save_share(&out, &share, &passphrase)?;
            println!("{}", serde_json::to_string_pretty(&pubkey_json(&share))?);
        }
        Cmd::Pubkey { share } => {
            let passphrase = store::passphrase_from_env()?;
            let share = store::load_share(&share, &passphrase)?;
            println!("{}", serde_json::to_string_pretty(&pubkey_json(&share))?);
        }
        Cmd::Serve {
            share,
            peers,
            listen,
            http,
            signers,
            timeout_secs,
            policy_rpc,
        } => {
            let passphrase = store::passphrase_from_env()?;
            let secret = secret_from_env()?;
            let header = store::read_header(&share)?;
            let share = store::load_share(&share, &passphrase)?;
            if peers.len() != header.n as usize {
                bail!("--peers must list exactly n={} addresses", header.n);
            }
            let transport = match listen {
                Some(l) => TcpTransport::bind_on(l, header.index, peers, secret)?,
                None => TcpTransport::bind(header.index, peers, secret)?,
            };
            let signers =
                signers.unwrap_or_else(|| default_signers(header.index, header.n, header.t));
            if signers.len() != header.t as usize || !signers.contains(&header.index) {
                bail!(
                    "--signers must name t={} parties including this one ({})",
                    header.t,
                    header.index
                );
            }
            let policy = match policy_rpc {
                Some(rpc) => Some(Arc::new(policy::Policy::new(&rpc, &share)?)),
                None => {
                    tracing::warn!("no --policy-rpc: this signer signs any digest it is asked");
                    None
                }
            };
            serve(
                share,
                transport,
                http,
                signers,
                Duration::from_secs(timeout_secs),
                policy,
            )?;
        }
    }
    Ok(())
}

fn pubkey_json(share: &KeyShare) -> serde_json::Value {
    serde_json::json!({
        "public_key": hex::encode(ecdsa::vault_public_key(share)),
        "chain_code": ecdsa::chain_code(share).map(hex::encode),
    })
}

fn default_signers(me: u16, n: u16, t: u16) -> Vec<u16> {
    let mut v = vec![me];
    v.extend((0..n).filter(|i| *i != me).take(t as usize - 1));
    v.sort_unstable();
    v
}

// ---------------------------------------------------------------- serve

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignRequest {
    digest: String,
    path: Vec<u32>,
    /// What the digest is for; required when a policy is configured.
    #[serde(default)]
    context: Option<SignContext>,
}

/// Control message the coordinator sends to start a signing session.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignAnnounce {
    session: String,
    digest: String,
    path: Vec<u32>,
    signers: Vec<u16>,
    #[serde(default)]
    context: Option<SignContext>,
}

struct LocalJob {
    digest: [u8; 32],
    path: Vec<u32>,
    context: Option<SignContext>,
    reply: tokio::sync::oneshot::Sender<Result<EcdsaSignature, String>>,
}

#[derive(Clone)]
struct HttpState {
    jobs: mpsc::Sender<LocalJob>,
    policy: Option<Arc<policy::Policy>>,
}

fn serve(
    share: KeyShare,
    transport: TcpTransport,
    http: SocketAddr,
    signers: Vec<u16>,
    timeout: Duration,
    policy: Option<Arc<policy::Policy>>,
) -> anyhow::Result<()> {
    let (jobs_tx, jobs_rx) = mpsc::channel::<LocalJob>();
    let share = Arc::new(share);
    let transport = Arc::new(transport);
    {
        let share = share.clone();
        let transport = transport.clone();
        let policy = policy.clone();
        thread::Builder::new()
            .name("keel-tss-coordinator".into())
            .spawn(move || coordinator(&share, &*transport, jobs_rx, signers, timeout, policy))?;
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let app = Router::new()
            .route("/sign", post(handle_sign))
            .with_state(HttpState {
                jobs: jobs_tx,
                policy,
            });
        let listener = tokio::net::TcpListener::bind(http).await?;
        tracing::info!(%http, "sign endpoint listening");
        axum::serve(listener, app).await?;
        Ok::<(), anyhow::Error>(())
    })
}

async fn handle_sign(
    State(st): State<HttpState>,
    Json(req): Json<SignRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let digest = match hex::decode(&req.digest)
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
    {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "digest must be 32 bytes hex" })),
            )
        }
    };
    if let Some(p) = &st.policy {
        if let Err(e) = p.check(&digest, &req.path, req.context.as_ref()) {
            tracing::warn!(error = %e, "signing request refused by policy");
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": format!("policy: {e}") })),
            );
        }
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    if st
        .jobs
        .send(LocalJob {
            digest,
            path: req.path,
            context: req.context,
            reply: tx,
        })
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "coordinator stopped" })),
        );
    }
    match rx.await {
        Ok(Ok(sig)) => (
            StatusCode::OK,
            Json(
                serde_json::json!({ "r": hex::encode(sig.r), "s": hex::encode(sig.s), "v": sig.v }),
            ),
        ),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "coordinator dropped the job" })),
        ),
    }
}

/// Runs signing sessions one at a time: local jobs (we coordinate) and
/// announcements from peers (we join).
fn coordinator(
    share: &KeyShare,
    transport: &dyn Transport,
    jobs: mpsc::Receiver<LocalJob>,
    signers: Vec<u16>,
    timeout: Duration,
    policy: Option<Arc<policy::Policy>>,
) {
    let mut mailbox = Mailbox::new(transport);
    let me = transport.my_index();
    let mut counter: u64 = 0;
    loop {
        match jobs.try_recv() {
            Ok(job) => {
                counter += 1;
                let mut nonce = [0u8; 8];
                rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
                let session_id = keel_crypto::sha256(&[
                    b"keel-tss-sign",
                    &job.digest,
                    &me.to_be_bytes(),
                    &counter.to_be_bytes(),
                    &nonce,
                ]);
                let announce = SignAnnounce {
                    session: hex::encode(session_id),
                    digest: hex::encode(job.digest),
                    path: job.path.clone(),
                    signers: signers.clone(),
                    context: job.context.clone(),
                };
                let body = serde_json::to_vec(&announce).unwrap_or_default();
                let mut announced = true;
                for s in signers.iter().filter(|s| **s != me) {
                    let m = WireMessage::p2p(CONTROL_SESSION, me, *s, body.clone());
                    if let Err(e) = transport.send(&m) {
                        tracing::error!(to = s, error = %e, "cannot reach signer");
                        announced = false;
                    }
                }
                let result = if announced {
                    run_sign(share, &mut mailbox, &announce, timeout)
                } else {
                    Err("announce failed".into())
                };
                let _ = job.reply.send(result);
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match mailbox.recv_session(CONTROL_SESSION, Duration::from_millis(200)) {
            Ok(m) => {
                let Ok(announce) = serde_json::from_slice::<SignAnnounce>(&m.body) else {
                    tracing::warn!(from = m.from, "bad announce");
                    continue;
                };
                if !announce.signers.contains(&me) || !announce.signers.contains(&m.from) {
                    tracing::warn!(
                        from = m.from,
                        "announce does not include this party or its sender"
                    );
                    continue;
                }
                if let Some(p) = &policy {
                    let digest = hex::decode(&announce.digest)
                        .ok()
                        .and_then(|v| <[u8; 32]>::try_from(v).ok());
                    let verdict = match digest {
                        Some(d) => p.check(&d, &announce.path, announce.context.as_ref()),
                        None => Err("bad digest".into()),
                    };
                    if let Err(e) = verdict {
                        tracing::warn!(from = m.from, session = %announce.session, error = %e, "announced session refused by policy");
                        continue;
                    }
                }
                match run_sign(share, &mut mailbox, &announce, timeout) {
                    Ok(sig) => {
                        tracing::info!(session = %announce.session, v = sig.v, "joined signing session")
                    }
                    Err(e) => {
                        tracing::error!(session = %announce.session, error = %e, "signing session failed")
                    }
                }
            }
            Err(TransportError::Timeout) => {}
            Err(e) => {
                tracing::error!(error = %e, "transport failed");
                return;
            }
        }
    }
}

fn run_sign(
    share: &KeyShare,
    mailbox: &mut Mailbox<'_>,
    a: &SignAnnounce,
    timeout: Duration,
) -> Result<EcdsaSignature, String> {
    let digest: [u8; 32] = hex::decode(&a.digest)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| "bad digest".to_string())?;
    let session_id = hex::decode(&a.session).map_err(|e| e.to_string())?;
    ecdsa::sign_with_mailbox(
        share,
        &a.path,
        digest,
        &a.signers,
        &session_id,
        mailbox,
        timeout,
    )
    .map_err(|e| e.to_string())
}
