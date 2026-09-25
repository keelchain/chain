//! `keel-indexer`: follow an Keel node, keep full history in Postgres, serve
//! the explorer API (`docs/explorer-api.md`).
//!
//! ```text
//! keel-indexer --network testnet --node-rpc http://127.0.0.1:5000 \
//!   --database-url postgres://keel:keel@localhost:5434/keel_indexer_testnet --listen 127.0.0.1:6100
//! ```

#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use anyhow::{Context, Result};
use clap::Parser;
use keel_indexer::api::{router, AppState};
use keel_indexer::db::Db;
use keel_indexer::node::{HttpNodeClient, Node};
use keel_indexer::sync::Indexer;
use keel_indexer::{Config, SyncStatus};
use std::sync::Arc;
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "keel-indexer",
    version,
    about = "Keelchain indexer + explorer API"
)]
struct Cli {
    /// Network label reported on /v1/health (`testnet`, `mainnet`, `devnet`).
    #[arg(long, env = "KEEL_NETWORK", default_value = "testnet")]
    network: String,
    /// Node RPC base URL.
    #[arg(long, env = "KEEL_NODE_RPC", default_value = "http://127.0.0.1:5000")]
    node_rpc: String,
    /// Node WebSocket URL (defaults to `<node-rpc>/v1/ws` with the ws scheme).
    #[arg(long, env = "KEEL_NODE_WS")]
    node_ws: Option<String>,
    /// Postgres URL.
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    /// API listen address.
    #[arg(long, env = "KEEL_INDEXER_LISTEN", default_value = "127.0.0.1:6100")]
    listen: String,
    /// External network label for links (`regtest`, `testnet`, `mainnet`).
    #[arg(long, env = "KEEL_EXTERNAL_NETWORK", default_value = "testnet")]
    external_network: String,
    /// Backfill fetch concurrency.
    #[arg(long, default_value_t = 16)]
    concurrency: usize,
    /// Blocks per write transaction during backfill.
    #[arg(long, default_value_t = 256)]
    batch: usize,
    /// Re-scan for missing blocks from this height before following.
    #[arg(long)]
    backfill_from: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    let cfg = Config {
        network: cli.network,
        node_ws: cli
            .node_ws
            .unwrap_or_else(|| Config::ws_url_for(&cli.node_rpc)),
        node_rpc: cli.node_rpc,
        database_url: cli.database_url,
        listen: cli.listen,
        external_network: cli.external_network,
        concurrency: cli.concurrency.max(1),
        batch: cli.batch.max(1),
        backfill_from: cli.backfill_from,
    };
    let db = Db::connect(&cfg.database_url).await?;
    let node: Node = Arc::new(HttpNodeClient::new(&cfg.node_rpc, &cfg.node_ws)?);
    let status = SyncStatus::new();
    let indexer = Indexer::new(cfg.clone(), db.clone(), node.clone(), status.clone());
    indexer.init().await.context("indexer init")?;
    let app = Arc::new(AppState {
        cfg: cfg.clone(),
        db,
        node,
        status,
        ws_tx: indexer.ws_tx.clone(),
    });
    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("bind {}", cfg.listen))?;
    info!(listen = %cfg.listen, node = %cfg.node_rpc, network = %cfg.network, "explorer API listening");
    let sync = tokio::spawn(indexer.run());
    let serve = axum::serve(listener, router(app));
    tokio::select! {
        r = serve => r.context("api server")?,
        r = sync => r.context("sync task")??,
    }
    Ok(())
}
