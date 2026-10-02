//! Keelchain indexer: follows a node (`/v1/blocks/{h}`, `/v1/blocks/{h}/receipts`,
//! `/v1/blocks/{h}/actions`, `/v1/ws`), keeps full history in Postgres and
//! serves the explorer API contract (`docs/explorer-api.md`).
//!
//! This crate sits ABOVE the VM (dev-rules.md applies to `keel-vm` and below):
//! tokio, HashMap and the wall clock are fine here, hence the allow below.
#![forbid(unsafe_code)]
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::float_arithmetic
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod api;
pub mod db;
pub mod materialize;
pub mod node;
pub mod sync;
pub mod types;

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

/// Runtime configuration (CLI flags in `main.rs`).
#[derive(Clone, Debug)]
pub struct Config {
    pub network: String,
    pub node_rpc: String,
    pub node_ws: String,
    pub database_url: String,
    pub listen: String,
    pub external_network: String,
    /// Backfill fetch concurrency (blocks in flight).
    pub concurrency: usize,
    /// Blocks per write transaction during backfill.
    pub batch: usize,
    /// Re-scan for holes from this height (fills blocks the node did not
    /// serve on an earlier run).
    pub backfill_from: Option<u64>,
    /// The testnet faucet (`POST /v1/faucet`); None when no key is configured.
    pub faucet: Option<FaucetConfig>,
}

/// Faucet settings (`KEEL_FAUCET_*`). Amounts are whole coins; the chain
/// uses six decimals for both KEEL and the stable.
#[derive(Clone, Debug)]
pub struct FaucetConfig {
    pub secret: [u8; 32],
    pub keel: u64,
    pub kusd: u64,
    /// Seconds an address waits between claims.
    pub cooldown_secs: i64,
    /// Claims one IP may make per day.
    pub ip_per_day: i64,
    /// Explorer base for the links in the response (`https://testnet.keelchain.com`).
    pub explorer_url: String,
}

impl Config {
    pub fn ws_url_for(rpc: &str) -> String {
        let base = rpc.trim_end_matches('/');
        let ws = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            format!("ws://{base}")
        };
        format!("{ws}/v1/ws")
    }
}

/// Live sync status shared between the sync loop and `/v1/health`.
#[derive(Default)]
pub struct SyncStatus {
    pub chain_id: AtomicU64,
    pub indexed_height: AtomicU64,
    pub node_height: AtomicU64,
    pub first_indexed_height: AtomicU64,
    pub missing_blocks: AtomicU64,
    pub state_hash_ok: AtomicBool,
    pub started_at: AtomicI64,
    pub backfilling: AtomicBool,
    pub mismatch: Mutex<Option<HashMismatch>>,
    /// Heights whose blocks are unavailable from the node (retention window).
    pub last_error: Mutex<Option<String>>,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct HashMismatch {
    pub height: u64,
    pub node: String,
    pub stored: String,
}

impl SyncStatus {
    pub fn new() -> Arc<Self> {
        let s = Self::default();
        s.state_hash_ok.store(true, Ordering::Relaxed);
        s.started_at.store(now_ms(), Ordering::Relaxed);
        Arc::new(s)
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
