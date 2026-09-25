//! Sync loop: backfill over HTTP (concurrent fetch, ordered writes, one
//! transaction per block range), then follow `/v1/ws`; on a gap or a
//! reconnect, backfill again. Periodic jobs keep node snapshots
//! (markets, validators, balances) fresh and verify the state-hash chain.

use crate::db::{Db, WriteOutcome};
use crate::materialize::{collect_refs, materialize, Materialized};
use crate::node::{fetch_actions, fetch_block, status as node_status, Node, RouteProbe};
use crate::types::{field_u64, BlockData};
use crate::{now_ms, Config, HashMismatch, SyncStatus};
use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

pub struct Indexer {
    pub cfg: Config,
    pub db: Db,
    pub node: Node,
    pub status: Arc<SyncStatus>,
    pub probe: RouteProbe,
    /// Explorer WebSocket fan-out (`{type:"block"|"tx", ...}` JSON).
    pub ws_tx: broadcast::Sender<String>,
    /// Latest `/v1/status` sample, checked against the stored hash once the
    /// block at that height is indexed.
    pending_verify: Mutex<Option<(u64, String)>>,
    /// Cached `/v1/params` (seeds `from` values of ParamChanged).
    pub params: Mutex<Value>,
    last_reconciled_height: Mutex<i64>,
}

impl Indexer {
    pub fn new(cfg: Config, db: Db, node: Node, status: Arc<SyncStatus>) -> Arc<Self> {
        let (ws_tx, _) = broadcast::channel(4096);
        Arc::new(Self {
            cfg,
            db,
            node,
            status,
            probe: RouteProbe::default(),
            ws_tx,
            pending_verify: Mutex::new(None),
            params: Mutex::new(Value::Null),
            last_reconciled_height: Mutex::new(0),
        })
    }

    /// Migrations, sync state, chain id, snapshots. Fails when the node is
    /// unreachable so a misconfiguration is visible at start.
    pub async fn init(&self) -> Result<()> {
        self.db.migrate().await?;
        let st = self.db.load_sync_state(&self.cfg.network).await?;
        self.status
            .indexed_height
            .store(st.indexed_height as u64, Ordering::Relaxed);
        self.status.first_indexed_height.store(
            st.first_indexed_height.unwrap_or(0) as u64,
            Ordering::Relaxed,
        );
        if let Some((h, node, stored)) = st.hash_mismatch {
            self.status.state_hash_ok.store(false, Ordering::Relaxed);
            *self.status.mismatch.lock().expect("lock") = Some(HashMismatch {
                height: h as u64,
                node,
                stored,
            });
        }
        let ns = node_status(&*self.node).await.context("node status")?;
        if let Some(cid) = st.chain_id {
            if cid as u64 != ns.chain_id {
                anyhow::bail!("database was indexed from chain_id {cid} but the node reports chain_id {} (wrong --network / --node-rpc?)", ns.chain_id);
            }
        } else {
            self.db
                .set_chain_id(&self.cfg.network, ns.chain_id as i64)
                .await?;
        }
        self.status.chain_id.store(ns.chain_id, Ordering::Relaxed);
        self.status.node_height.store(ns.height, Ordering::Relaxed);
        self.db.seed_assets(now_ms()).await?;
        self.refresh_params().await;
        self.snapshot_markets().await;
        self.snapshot_validators().await;
        info!(network = %self.cfg.network, chain_id = ns.chain_id, node_height = ns.height, indexed = st.indexed_height, "indexer initialised");
        Ok(())
    }

    /// Main loop: backfill to the tip, follow the WebSocket, repeat on drop.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let me = self.clone();
        tokio::spawn(async move { me.periodic().await });
        if let Some(from) = self.cfg.backfill_from {
            self.fill_holes(from).await;
        }
        loop {
            if let Err(e) = self.catch_up().await {
                warn!(%e, "backfill failed; retrying in 2s");
                *self.status.last_error.lock().expect("lock") = Some(e.to_string());
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            match self.follow().await {
                Ok(()) => warn!("websocket closed; reconnecting"),
                Err(e) => {
                    warn!(%e, "websocket error; reconnecting");
                    *self.status.last_error.lock().expect("lock") = Some(e.to_string());
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    /// Backfill from `indexed_height + 1` to the node's tip, looping until
    /// the tip stops moving away.
    async fn catch_up(&self) -> Result<()> {
        loop {
            let ns = node_status(&*self.node).await?;
            self.status.node_height.store(ns.height, Ordering::Relaxed);
            let from = self.status.indexed_height.load(Ordering::Relaxed) + 1;
            if from > ns.height {
                return Ok(());
            }
            self.status.backfilling.store(true, Ordering::Relaxed);
            let t0 = Instant::now();
            let written = self.backfill(from, ns.height).await;
            self.status.backfilling.store(false, Ordering::Relaxed);
            let written = written?;
            let secs = t0.elapsed().as_secs_f64().max(1e-3);
            info!(
                from,
                to = ns.height,
                written,
                blocks_per_s = format!("{:.0}", written as f64 / secs),
                "backfill range done"
            );
            if ns.height.saturating_sub(from) < 16 {
                return Ok(());
            }
        }
    }

    /// Fetch `[from, to]` with `cfg.concurrency` requests in flight, write in
    /// order in batches of `cfg.batch`. Heights the node no longer serves are
    /// skipped and counted (`missing_blocks` on `/v1/health`).
    pub async fn backfill(&self, from: u64, to: u64) -> Result<u64> {
        if from > to {
            return Ok(0);
        }
        let mut from = from;
        if fetch_block(&*self.node, &self.probe, from).await?.is_none() {
            let oldest = self.oldest_available(from, to).await?;
            match oldest {
                Some(o) => {
                    let skipped = o - from;
                    warn!(from, oldest_available = o, skipped, "node does not serve the oldest requested blocks; starting at the oldest it has");
                    self.status
                        .missing_blocks
                        .fetch_add(skipped, Ordering::Relaxed);
                    from = o;
                }
                None => return Ok(0),
            }
        }
        let node = self.node.clone();
        let probe = &self.probe;
        let mut stream = futures::stream::iter(from..=to)
            .map(|h| {
                let node = node.clone();
                async move { (h, fetch_block(&*node, probe, h).await) }
            })
            .buffered(self.cfg.concurrency.max(1));
        let mut batch: Vec<BlockData> = Vec::with_capacity(self.cfg.batch);
        let mut written = 0u64;
        let mut last_log = Instant::now();
        while let Some((h, res)) = stream.next().await {
            match res {
                Ok(Some(b)) => batch.push(b),
                Ok(None) => {
                    self.status.missing_blocks.fetch_add(1, Ordering::Relaxed);
                    debug!(height = h, "block unavailable from node");
                }
                Err(e) => return Err(e.context(format!("fetch block {h}"))),
            }
            if batch.len() >= self.cfg.batch.max(1) {
                let out = self.write_blocks(std::mem::take(&mut batch)).await?;
                written += out.new_heights as u64;
                if last_log.elapsed() > Duration::from_secs(5) {
                    info!(height = h, to, written, "backfilling");
                    last_log = Instant::now();
                }
            }
        }
        if !batch.is_empty() {
            written += self.write_blocks(batch).await?.new_heights as u64;
        }
        Ok(written)
    }

    /// Binary search for the lowest height in `(from, to]` the node still
    /// serves (availability is a suffix of the chain: a retention window).
    async fn oldest_available(&self, from: u64, to: u64) -> Result<Option<u64>> {
        if fetch_block(&*self.node, &self.probe, to).await?.is_none() {
            return Ok(None);
        }
        let (mut lo, mut hi) = (from + 1, to);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if fetch_block(&*self.node, &self.probe, mid).await?.is_some() {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        Ok(Some(lo))
    }

    /// `--backfill-from`: re-fetch every height without a block row.
    async fn fill_holes(&self, from: u64) {
        let indexed = self.status.indexed_height.load(Ordering::Relaxed);
        if indexed <= from {
            return;
        }
        let holes = match self.db.missing_heights(from as i64, indexed as i64).await {
            Ok(h) => h,
            Err(e) => {
                warn!(%e, "listing holes failed");
                return;
            }
        };
        info!(holes = holes.len(), from, to = indexed, "filling holes");
        let mut batch = Vec::new();
        for h in holes {
            match fetch_block(&*self.node, &self.probe, h as u64).await {
                Ok(Some(b)) => batch.push(b),
                Ok(None) => {}
                Err(e) => warn!(%e, height = h, "hole fetch failed"),
            }
            if batch.len() >= self.cfg.batch {
                if let Err(e) = self.write_blocks(std::mem::take(&mut batch)).await {
                    warn!(%e, "hole write failed");
                }
            }
        }
        if !batch.is_empty() {
            if let Err(e) = self.write_blocks(batch).await {
                warn!(%e, "hole write failed");
            }
        }
    }

    /// Follow `/v1/ws` until it drops. Messages are drained in small bursts
    /// so a fast chain (the devnet does ~45 blocks/s) gets one transaction
    /// per burst rather than per block.
    async fn follow(&self) -> Result<()> {
        let mut stream = self.node.subscribe().await?;
        info!(ws = %self.cfg.node_ws, "following websocket");
        while let Some(first) = stream.next().await {
            let mut blocks = vec![first];
            while blocks.len() < self.cfg.batch.max(1) {
                match tokio::time::timeout(Duration::from_millis(15), stream.next()).await {
                    Ok(Some(b)) => blocks.push(b),
                    _ => break,
                }
            }
            blocks.sort_by_key(|b| b.height);
            blocks.dedup_by_key(|b| b.height);
            let indexed = self.status.indexed_height.load(Ordering::Relaxed);
            let lowest = blocks[0].height;
            if indexed > 0 && lowest > indexed + 1 {
                info!(
                    from = indexed + 1,
                    to = lowest - 1,
                    "websocket gap; backfilling"
                );
                self.backfill(indexed + 1, lowest - 1).await?;
            }
            blocks.retain(|b| b.height > indexed);
            for b in &mut blocks {
                if let Err(e) = fetch_actions(&*self.node, &self.probe, b).await {
                    warn!(%e, height = b.height, "actions fetch failed");
                }
            }
            if !blocks.is_empty() {
                let top = blocks.last().map(|b| b.height).unwrap_or(0);
                self.write_blocks(blocks).await?;
                self.status.node_height.fetch_max(top, Ordering::Relaxed);
            }
        }
        Ok(())
    }

    /// Materialize and write a sorted run of blocks; then enrich touched
    /// records from the node, verify hashes and fan out to explorer clients.
    pub async fn write_blocks(&self, mut blocks: Vec<BlockData>) -> Result<WriteOutcome> {
        if blocks.is_empty() {
            return Ok(WriteOutcome {
                new_heights: 0,
                last_height: None,
                touched: Default::default(),
            });
        }
        blocks.sort_by_key(|b| b.height);
        let refs = collect_refs(&blocks);
        let mut ctx = self.db.preload_ctx(&refs).await?;
        Db::seed_params(&mut ctx, &self.params.lock().expect("lock"));
        let mut prev_ts = self
            .db
            .timestamp_before(blocks[0].height as i64)
            .await?
            .unwrap_or(0);
        let mut mats: Vec<Materialized> = Vec::with_capacity(blocks.len());
        for b in &blocks {
            let m = materialize(b, &mut ctx, prev_ts);
            prev_ts = m.block.timestamp;
            mats.push(m);
        }
        let summaries: Vec<Value> = mats.iter().map(block_summary).collect();
        let tx_msgs: Vec<Value> = mats
            .iter()
            .flat_map(|m| m.txs.iter().map(tx_summary))
            .collect();
        let out = self.db.write_batch(&self.cfg.network, mats).await?;
        if let Some(last) = out.last_height {
            self.status
                .indexed_height
                .fetch_max(last as u64, Ordering::Relaxed);
            let first = self.status.first_indexed_height.load(Ordering::Relaxed);
            let lowest = blocks[0].height;
            if first == 0 || lowest < first {
                self.status
                    .first_indexed_height
                    .store(lowest, Ordering::Relaxed);
            }
        }
        for b in &blocks {
            if let Some(h) = &b.state_hash {
                self.verify_hash(b.height, h).await;
            }
        }
        self.enrich(&out.touched).await;
        if self.ws_tx.receiver_count() > 0 {
            for s in summaries {
                let _ = self
                    .ws_tx
                    .send(json!({"type": "block", "block": s}).to_string());
            }
            for t in tx_msgs {
                let _ = self.ws_tx.send(json!({"type": "tx", "tx": t}).to_string());
            }
        }
        Ok(out)
    }

    /// The stored hash at `height` vs the node's `/v1/status` sample for the
    /// same height. A mismatch is sticky (persisted in `sync_state`).
    async fn verify_hash(&self, height: u64, stored: &str) {
        let pending = self.pending_verify.lock().expect("lock").clone();
        if let Some((h, node_hash)) = pending {
            if h == height && !node_hash.is_empty() && node_hash != stored {
                self.flag_mismatch(height, &node_hash, stored).await;
            }
        }
    }

    async fn flag_mismatch(&self, height: u64, node: &str, stored: &str) {
        error!(
            height,
            node,
            stored,
            "STATE HASH MISMATCH: node disagrees with the indexed chain (bug or different network)"
        );
        self.status.state_hash_ok.store(false, Ordering::Relaxed);
        *self.status.mismatch.lock().expect("lock") = Some(HashMismatch {
            height,
            node: node.into(),
            stored: stored.into(),
        });
        if let Err(e) = self
            .db
            .record_hash_mismatch(&self.cfg.network, height as i64, node, stored)
            .await
        {
            warn!(%e, "persisting mismatch failed");
        }
    }

    /// `/v1/status` sample: remember it for the block to come, or check it
    /// right away when that block is already stored.
    pub async fn check_state_hash(&self) {
        let Ok(ns) = node_status(&*self.node).await else {
            return;
        };
        self.status.node_height.store(ns.height, Ordering::Relaxed);
        *self.pending_verify.lock().expect("lock") = Some((ns.height, ns.state_hash.clone()));
        if let Ok(Some(Some(stored))) = self.db.block_state_hash(ns.height as i64).await {
            if !ns.state_hash.is_empty() && stored != ns.state_hash {
                self.flag_mismatch(ns.height, &ns.state_hash, &stored).await;
            }
        }
    }

    // ---------------- snapshots and enrichment ----------------

    async fn enrich(&self, t: &crate::materialize::Touched) {
        for id in &t.orders {
            if let Ok(Some(v)) = self.node.get(&format!("/v1/orders/{id}")).await {
                let _ = self
                    .db
                    .enrich_order(&v)
                    .await
                    .map_err(|e| warn!(%e, id, "enrich order"));
            }
        }
        for id in &t.offers {
            if let Ok(Some(v)) = self.node.get(&format!("/v1/offers/{id}")).await {
                let _ = self
                    .db
                    .enrich_offer(&v)
                    .await
                    .map_err(|e| warn!(%e, id, "enrich offer"));
            }
        }
        for id in &t.trades {
            if let Ok(Some(v)) = self.node.get(&format!("/v1/trades/{id}")).await {
                let _ = self
                    .db
                    .enrich_trade(&v)
                    .await
                    .map_err(|e| warn!(%e, id, "enrich trade"));
            }
        }
        for id in &t.proposals {
            if let Ok(Some(v)) = self.node.get(&format!("/v1/gov/proposals/{id}")).await {
                let _ = self
                    .db
                    .enrich_proposal(&v)
                    .await
                    .map_err(|e| warn!(%e, id, "enrich proposal"));
            }
        }
        if !t.outbounds.is_empty() {
            if let Ok(Some(v)) = self.node.get("/v1/vaults/outbounds").await {
                for o in v
                    .get("outbounds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if field_u64(o, "id").is_some_and(|id| t.outbounds.contains(&id)) {
                        let _ = self
                            .db
                            .enrich_outbound(o)
                            .await
                            .map_err(|e| warn!(%e, "enrich outbound"));
                    }
                }
            }
        }
        if t.epoch_advanced {
            self.snapshot_validators().await;
        }
        if t.asset_registered {
            let _ = self.db.seed_assets(now_ms()).await;
        }
    }

    pub async fn refresh_params(&self) {
        if let Ok(Some(p)) = self.node.get("/v1/params").await {
            *self.params.lock().expect("lock") = p;
        }
    }

    pub async fn snapshot_markets(&self) {
        match self.node.get("/v1/markets").await {
            Ok(Some(v)) => {
                if let Err(e) = self.db.upsert_markets(&v, now_ms()).await {
                    warn!(%e, "markets snapshot failed");
                }
            }
            Ok(None) => {}
            Err(e) => warn!(%e, "markets fetch failed"),
        }
    }

    pub async fn snapshot_validators(&self) {
        match self.node.get("/v1/staking/validators").await {
            Ok(Some(v)) => {
                let h = self.status.indexed_height.load(Ordering::Relaxed) as i64;
                if let Err(e) = self.db.upsert_validators(&v, h, now_ms()).await {
                    warn!(%e, "validators snapshot failed");
                }
            }
            Ok(None) => {}
            Err(e) => warn!(%e, "validators fetch failed"),
        }
    }

    /// Balances of every address seen since the last run (or all of them)
    /// from the node, then supply / holder counts per asset.
    pub async fn reconcile_balances(&self, full: bool) {
        let since = if full {
            None
        } else {
            Some(*self.last_reconciled_height.lock().expect("lock"))
        };
        let top = self.status.indexed_height.load(Ordering::Relaxed) as i64;
        let mut addrs = match self.db.addresses(since).await {
            Ok(a) => a,
            Err(e) => {
                warn!(%e, "listing addresses failed");
                return;
            }
        };
        // The system address holds the vault reserves and the issuance
        // counters: refresh it on every pass so reserves/supply never lag.
        addrs.push(crate::materialize::SYSTEM_ADDR.to_string());
        addrs.sort();
        addrs.dedup();
        let now = now_ms();
        let node = self.node.clone();
        let results = futures::stream::iter(addrs)
            .map(|a| {
                let node = node.clone();
                async move {
                    let v = node.get(&format!("/v1/accounts/{a}")).await;
                    (a, v)
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        for (a, v) in results {
            match v {
                Ok(Some(acc)) => {
                    let balances = acc
                        .get("balances")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if let Err(e) = self.db.replace_balances(&a, &balances, now).await {
                        warn!(%e, address = a, "balance write failed");
                    }
                }
                Ok(None) => {}
                Err(e) => debug!(%e, address = a, "account fetch failed"),
            }
        }
        if let Err(e) = self.db.refresh_asset_stats(now).await {
            warn!(%e, "asset stats failed");
        }
        *self.last_reconciled_height.lock().expect("lock") = top;
    }

    async fn periodic(&self) {
        let mut tick = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tick += 5;
            self.check_state_hash().await;
            if tick.is_multiple_of(30) {
                self.refresh_params().await;
                self.snapshot_markets().await;
                if !self.status.backfilling.load(Ordering::Relaxed) {
                    self.reconcile_balances(tick.is_multiple_of(600)).await;
                }
            }
            if tick.is_multiple_of(60) {
                self.snapshot_validators().await;
            }
        }
    }
}

pub fn block_summary(m: &Materialized) -> Value {
    let b = &m.block;
    json!({
        "height": b.height,
        "timestamp": b.timestamp,
        "timestamp_exact": b.timestamp_exact,
        "state_hash": b.state_hash,
        "tx_count": b.tx_count,
        "ok_count": b.ok_count,
        "event_count": b.event_count,
        "proposer": b.proposer,
    })
}

pub fn tx_summary(t: &crate::materialize::TxRow) -> Value {
    json!({
        "tx_id": t.tx_id,
        "height": t.height,
        "index": t.index,
        "timestamp": t.timestamp,
        "signer": t.signer,
        "module": t.module,
        "kind": t.kind,
        "ok": t.ok,
        "error": t.error_code.as_ref().map(|c| json!({"code": c, "message": t.error_message})),
        "event_count": t.event_count,
    })
}
