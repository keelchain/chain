//! `keel-tss`: the signer daemon of one observer.
//!
//! ```text
//! keel-tss gen-primes --out primes.json
//! keel-tss keygen --index 0 --n 4 --t 3 --peers h0:7000,h1:7000,h2:7000,h3:7000 \
//!         --eid keel-vault:BTC:epoch:1 --out share.enc [--primes primes.json]
//! keel-tss keygen --scheme eddsa --index 0 --n 4 --t 3 --peers ... \
//!         --eid keel-vault:SOL:epoch:1 --out share-sol.enc
//! keel-tss pubkey --share share.enc
//! keel-tss serve  --share share.enc [--eddsa-share share-sol.enc] --listen 127.0.0.1:7000 \
//!         --peers ... --http 127.0.0.1:7100
//! ```
//!
//! Environment: `KEEL_TSS_PASSPHRASE` (share files), `KEEL_TSS_SECRET` (hex,
//! HMAC key shared by the signer set for the TCP transport).
//!
//! `serve` exposes `POST /sign {"digest": hex32, "path": [u32, ...],
//! "context": {...}}` → `{"r": hex, "s": hex, "v": 0|1}` for the secp256k1
//! chains (CGGMP21), and `{"scheme": "eddsa", "digest": sha256(message),
//! "message": hex, "path": [...], "context": {...}}` → `{"signature":
//! hex64}` for Solana (FROST), for the local observer daemon only (bind it
//! to loopback). The receiving party coordinates: it picks the signer
//! subset, sends a `control` announcement (digest, path and context) to
//! every signer and each of them joins the session. With `--policy-rpc
//! <node>` every party, coordinator and joiner, first checks the context
//! against the chain's outbound rows (`keel_chains::policy`): the digest
//! must be the signing hash of a transaction that pays open outbounds of
//! the named batch, from the vault's own keys. Without the flag (devnet)
//! any digest is signed.
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::{bail, Context as _};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use clap::{Parser, Subcommand};
use keel_chains::policy::SignContext;
use keel_tss::{
    ecdsa::{self, EcdsaSignature, KeyShare, KeygenParams, Primes},
    eddsa::{self, EddsaShare},
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
        /// `ecdsa` (CGGMP21 on secp256k1: Bitcoin, Ethereum, BSC, Tron) or
        /// `eddsa` (FROST on Ed25519: Solana).
        #[arg(long, default_value = "ecdsa")]
        scheme: String,
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
        /// Paillier primes (ecdsa only).
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
        /// The CGGMP21 share (secp256k1 chains).
        #[arg(long)]
        share: Option<PathBuf>,
        /// The FROST share (Solana). At least one share is needed; both
        /// must come from the same party set (index and n).
        #[arg(long)]
        eddsa_share: Option<PathBuf>,
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
            scheme,
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
            let transport = match listen {
                Some(l) => TcpTransport::bind_on(l, index, peers, secret)?,
                None => TcpTransport::bind(index, peers, secret)?,
            };
            match scheme.as_str() {
                "ecdsa" => {
                    let primes = match primes {
                        Some(p) => Some(
                            serde_json::from_slice::<Primes>(&std::fs::read(&p)?)
                                .context("primes file")?,
                        ),
                        None => None,
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
                "eddsa" => {
                    let params = eddsa::KeygenParams {
                        t,
                        n,
                        my_index: index,
                        execution_id: eid.into_bytes(),
                        timeout: Duration::from_secs(timeout_secs),
                    };
                    let share = eddsa::keygen(params, &transport)?;
                    store::save_eddsa_share(&out, &share, &passphrase)?;
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&eddsa_pubkey_json(&share))?
                    );
                }
                other => bail!("--scheme must be ecdsa or eddsa, not {other}"),
            }
        }
        Cmd::Pubkey { share } => {
            let passphrase = store::passphrase_from_env()?;
            let header = store::read_header(&share)?;
            let v = match header.scheme() {
                "eddsa" => eddsa_pubkey_json(&store::load_eddsa_share(&share, &passphrase)?),
                _ => pubkey_json(&store::load_share(&share, &passphrase)?),
            };
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        Cmd::Serve {
            share,
            eddsa_share,
            peers,
            listen,
            http,
            signers,
            timeout_secs,
            policy_rpc,
        } => {
            let passphrase = store::passphrase_from_env()?;
            let secret = secret_from_env()?;
            let Some(first) = share.as_ref().or(eddsa_share.as_ref()) else {
                bail!("pass --share (ecdsa), --eddsa-share, or both");
            };
            let header = store::read_header(first)?;
            let ecdsa_share = match &share {
                Some(p) => Some(Arc::new(store::load_share(p, &passphrase)?)),
                None => None,
            };
            let eddsa_share = match &eddsa_share {
                Some(p) => {
                    let h = store::read_header(p)?;
                    if h.index != header.index || h.n != header.n {
                        bail!("the two shares belong to different party sets");
                    }
                    Some(Arc::new(store::load_eddsa_share(p, &passphrase)?))
                }
                None => None,
            };
            if peers.len() != header.n as usize {
                bail!("--peers must list exactly n={} addresses", header.n);
            }
            let transport = match listen {
                Some(l) => TcpTransport::bind_on(l, header.index, peers, secret)?,
                None => TcpTransport::bind(header.index, peers, secret)?,
            };
            let subset = |t: u16| -> anyhow::Result<Vec<u16>> {
                let s = signers
                    .clone()
                    .unwrap_or_else(|| default_signers(header.index, header.n, t));
                if s.len() != t as usize || !s.contains(&header.index) {
                    bail!(
                        "--signers must name t={t} parties including this one ({})",
                        header.index
                    );
                }
                Ok(s)
            };
            let signer_sets = SignerSets {
                ecdsa: match &ecdsa_share {
                    Some(s) => Some(subset(header_t(s))?),
                    None => None,
                },
                eddsa: match &eddsa_share {
                    Some(s) => Some(subset(s.t)?),
                    None => None,
                },
            };
            let policy = match policy_rpc {
                Some(rpc) => Some(Arc::new(policy::Policy::new(
                    &rpc,
                    ecdsa_share.as_deref(),
                    eddsa_share.as_deref(),
                )?)),
                None => {
                    tracing::warn!("no --policy-rpc: this signer signs any digest it is asked");
                    None
                }
            };
            serve(
                Shares {
                    ecdsa: ecdsa_share,
                    eddsa: eddsa_share,
                },
                transport,
                http,
                signer_sets,
                Duration::from_secs(timeout_secs),
                policy,
            )?;
        }
    }
    Ok(())
}

fn header_t(share: &KeyShare) -> u16 {
    use cggmp21::key_share::AnyKeyShare as _;
    share.min_signers()
}

fn pubkey_json(share: &KeyShare) -> serde_json::Value {
    serde_json::json!({
        "scheme": "ecdsa",
        "public_key": hex::encode(ecdsa::vault_public_key(share)),
        "chain_code": ecdsa::chain_code(share).map(hex::encode),
    })
}

fn eddsa_pubkey_json(share: &EddsaShare) -> serde_json::Value {
    serde_json::json!({
        "scheme": "eddsa",
        "public_key": hex::encode(eddsa::vault_public_key(share)),
        "chain_code": hex::encode(eddsa::chain_code(share)),
    })
}

fn default_signers(me: u16, n: u16, t: u16) -> Vec<u16> {
    let mut v = vec![me];
    v.extend((0..n).filter(|i| *i != me).take(t as usize - 1));
    v.sort_unstable();
    v
}

// ---------------------------------------------------------------- serve

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Scheme {
    #[default]
    Ecdsa,
    Eddsa,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignRequest {
    /// The 32-byte digest to sign (ecdsa), or the SHA-256 of `message`
    /// (eddsa), hex.
    digest: String,
    path: Vec<u32>,
    /// What the digest is for; required when a policy is configured.
    #[serde(default)]
    context: Option<SignContext>,
    #[serde(default)]
    scheme: Scheme,
    /// The bytes an eddsa request signs, hex.
    #[serde(default)]
    message: Option<String>,
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
    #[serde(default)]
    scheme: Scheme,
    #[serde(default)]
    message: Option<String>,
}

enum SigOut {
    Ecdsa(EcdsaSignature),
    Eddsa([u8; 64]),
}

struct LocalJob {
    scheme: Scheme,
    digest: [u8; 32],
    message: Option<Vec<u8>>,
    path: Vec<u32>,
    context: Option<SignContext>,
    reply: tokio::sync::oneshot::Sender<Result<SigOut, String>>,
}

#[derive(Clone)]
struct HttpState {
    jobs: mpsc::Sender<LocalJob>,
    policy: Option<Arc<policy::Policy>>,
    has_ecdsa: bool,
    has_eddsa: bool,
}

struct Shares {
    ecdsa: Option<Arc<KeyShare>>,
    eddsa: Option<Arc<EddsaShare>>,
}

struct SignerSets {
    ecdsa: Option<Vec<u16>>,
    eddsa: Option<Vec<u16>>,
}

fn serve(
    shares: Shares,
    transport: TcpTransport,
    http: SocketAddr,
    signers: SignerSets,
    timeout: Duration,
    policy: Option<Arc<policy::Policy>>,
) -> anyhow::Result<()> {
    let (jobs_tx, jobs_rx) = mpsc::channel::<LocalJob>();
    let transport = Arc::new(transport);
    let has_ecdsa = shares.ecdsa.is_some();
    let has_eddsa = shares.eddsa.is_some();
    {
        let transport = transport.clone();
        let policy = policy.clone();
        thread::Builder::new()
            .name("keel-tss-coordinator".into())
            .spawn(move || coordinator(&shares, &*transport, jobs_rx, signers, timeout, policy))?;
    }
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let app = Router::new()
            .route("/sign", post(handle_sign))
            .with_state(HttpState {
                jobs: jobs_tx,
                policy,
                has_ecdsa,
                has_eddsa,
            });
        let listener = tokio::net::TcpListener::bind(http).await?;
        tracing::info!(%http, ecdsa = has_ecdsa, eddsa = has_eddsa, "sign endpoint listening");
        axum::serve(listener, app).await?;
        Ok::<(), anyhow::Error>(())
    })
}

/// The digest and, for eddsa, the message bytes it must be the hash of.
fn request_parts(
    digest: &str,
    scheme: Scheme,
    message: Option<&str>,
) -> Result<([u8; 32], Option<Vec<u8>>), String> {
    let digest: [u8; 32] = hex::decode(digest)
        .ok()
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| "digest must be 32 bytes hex".to_string())?;
    match scheme {
        Scheme::Ecdsa => Ok((digest, None)),
        Scheme::Eddsa => {
            let m = hex::decode(message.ok_or("an eddsa request carries the message")?)
                .map_err(|e| format!("message: {e}"))?;
            if keel_crypto::sha256(&[&m]) != digest {
                return Err("digest is not the hash of the message".into());
            }
            Ok((digest, Some(m)))
        }
    }
}

async fn handle_sign(
    State(st): State<HttpState>,
    Json(req): Json<SignRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let bad = |e: String| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
    };
    let (digest, message) = match request_parts(&req.digest, req.scheme, req.message.as_deref()) {
        Ok(p) => p,
        Err(e) => return bad(e),
    };
    let served = match req.scheme {
        Scheme::Ecdsa => st.has_ecdsa,
        Scheme::Eddsa => st.has_eddsa,
    };
    if !served {
        return bad(format!("this signer holds no {:?} share", req.scheme).to_lowercase());
    }
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
            scheme: req.scheme,
            digest,
            message,
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
        Ok(Ok(SigOut::Ecdsa(sig))) => (
            StatusCode::OK,
            Json(
                serde_json::json!({ "r": hex::encode(sig.r), "s": hex::encode(sig.s), "v": sig.v }),
            ),
        ),
        Ok(Ok(SigOut::Eddsa(sig))) => (
            StatusCode::OK,
            Json(serde_json::json!({ "signature": hex::encode(sig) })),
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
    shares: &Shares,
    transport: &dyn Transport,
    jobs: mpsc::Receiver<LocalJob>,
    signers: SignerSets,
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
                let set = match job.scheme {
                    Scheme::Ecdsa => signers.ecdsa.clone(),
                    Scheme::Eddsa => signers.eddsa.clone(),
                };
                let Some(set) = set else {
                    let _ = job.reply.send(Err("no share for that scheme".into()));
                    continue;
                };
                let announce = SignAnnounce {
                    session: hex::encode(session_id),
                    digest: hex::encode(job.digest),
                    path: job.path.clone(),
                    signers: set.clone(),
                    context: job.context.clone(),
                    scheme: job.scheme,
                    message: job.message.as_ref().map(hex::encode),
                };
                let body = serde_json::to_vec(&announce).unwrap_or_default();
                let mut announced = true;
                for s in set.iter().filter(|s| **s != me) {
                    let m = WireMessage::p2p(CONTROL_SESSION, me, *s, body.clone());
                    if let Err(e) = transport.send(&m) {
                        tracing::error!(to = s, error = %e, "cannot reach signer");
                        announced = false;
                    }
                }
                let result = if announced {
                    run_sign(shares, &mut mailbox, &announce, timeout)
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
                let parts = request_parts(
                    &announce.digest,
                    announce.scheme,
                    announce.message.as_deref(),
                );
                let digest = match parts {
                    Ok((d, _)) => d,
                    Err(e) => {
                        tracing::warn!(from = m.from, error = %e, "bad announce");
                        continue;
                    }
                };
                if let Some(p) = &policy {
                    if let Err(e) = p.check(&digest, &announce.path, announce.context.as_ref()) {
                        tracing::warn!(from = m.from, session = %announce.session, error = %e, "announced session refused by policy");
                        continue;
                    }
                }
                match run_sign(shares, &mut mailbox, &announce, timeout) {
                    Ok(_) => {
                        tracing::info!(session = %announce.session, scheme = ?announce.scheme, "joined signing session")
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
    shares: &Shares,
    mailbox: &mut Mailbox<'_>,
    a: &SignAnnounce,
    timeout: Duration,
) -> Result<SigOut, String> {
    let (digest, message) = request_parts(&a.digest, a.scheme, a.message.as_deref())?;
    let session_id = hex::decode(&a.session).map_err(|e| e.to_string())?;
    match a.scheme {
        Scheme::Ecdsa => {
            let share = shares.ecdsa.as_ref().ok_or("no ecdsa share")?;
            ecdsa::sign_with_mailbox(
                share,
                &a.path,
                digest,
                &a.signers,
                &session_id,
                mailbox,
                timeout,
            )
            .map(SigOut::Ecdsa)
            .map_err(|e| e.to_string())
        }
        Scheme::Eddsa => {
            let share = shares.eddsa.as_ref().ok_or("no eddsa share")?;
            let message = message.ok_or("no message")?;
            eddsa::sign_with_mailbox(
                share,
                &a.path,
                &message,
                &a.signers,
                &session_id,
                mailbox,
                timeout,
            )
            .map(SigOut::Eddsa)
            .map_err(|e| e.to_string())
        }
    }
}
