//! Node access: one small trait (`get` + `subscribe`) so the sync loop and
//! the API are testable against a mock. `fetch_block` layers the documented
//! routes with fallbacks for nodes that lack some of them.

use crate::types::{
    parse_actions, parse_block_update, parse_receipt, parse_status, BlockData, NodeStatus,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[async_trait]
pub trait NodeClient: Send + Sync + 'static {
    /// `POST` a JSON body (action submission); the node's JSON answer or an
    /// error carrying its body.
    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let _ = (path, body);
        Err(anyhow!("this node client cannot submit actions"))
    }
    /// `GET {rpc}{path}`; `Ok(None)` on 404.
    async fn get(&self, path: &str) -> Result<Option<Value>>;
    /// Live blocks from `/v1/ws`; the stream ends when the socket drops.
    async fn subscribe(&self) -> Result<BoxStream<'static, BlockData>>;
}

pub type Node = Arc<dyn NodeClient>;

pub async fn status(node: &dyn NodeClient) -> Result<NodeStatus> {
    let v = node
        .get("/v1/status")
        .await?
        .ok_or_else(|| anyhow!("/v1/status not found"))?;
    parse_status(&v).ok_or_else(|| anyhow!("bad /v1/status body"))
}

/// Remembers which optional routes the node lacks so backfill does not pay
/// a 404 per block. Re-probed every `REPROBE` heights.
#[derive(Default)]
pub struct RouteProbe {
    blocks_missing: AtomicBool,
    actions_missing: AtomicBool,
    last_probe: AtomicU64,
}

const REPROBE: u64 = 5_000;

impl RouteProbe {
    fn should_try(&self, flag: &AtomicBool, height: u64) -> bool {
        if !flag.load(Ordering::Relaxed) {
            return true;
        }
        let last = self.last_probe.load(Ordering::Relaxed);
        if height >= last + REPROBE {
            self.last_probe.store(height, Ordering::Relaxed);
            return true;
        }
        false
    }
    pub fn blocks_route_missing(&self) -> bool {
        self.blocks_missing.load(Ordering::Relaxed)
    }
    pub fn actions_route_missing(&self) -> bool {
        self.actions_missing.load(Ordering::Relaxed)
    }
}

/// Everything the node knows about `height`, or `None` when it serves
/// nothing for it (outside its retention window / not yet produced).
///
/// Routes, in order: `/v1/blocks/{h}` (header, may inline receipts),
/// `/v1/blocks/{h}/receipts` (`{height, receipts, events}`),
/// `/v1/blocks/{h}/actions` (`{height, actions:[{index, tx_id, signer,
/// nonce, action}]}`). A missing blocks route falls back to the receipts
/// route (no timestamp for empty blocks, no state hash); a missing actions
/// route leaves `actions = None`.
pub async fn fetch_block(
    node: &dyn NodeClient,
    probe: &RouteProbe,
    height: u64,
) -> Result<Option<BlockData>> {
    let path = format!("/v1/blocks/{height}");
    let mut block: Option<BlockData> = None;
    if probe.should_try(&probe.blocks_missing, height) {
        if let Some(v) = node.get(&path).await? {
            probe.blocks_missing.store(false, Ordering::Relaxed);
            block = parse_block_update(&v);
        }
    }
    let need_receipts = block
        .as_ref()
        .is_none_or(|b| b.receipts.is_empty() && b.events.is_empty());
    if need_receipts {
        match node.get(&format!("{path}/receipts")).await? {
            Some(v) => {
                let receipts = v
                    .get("receipts")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(parse_receipt).collect())
                    .unwrap_or_default();
                let events = v
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                match block.as_mut() {
                    Some(b) => {
                        b.receipts = receipts;
                        b.events = events;
                    }
                    None => {
                        probe.blocks_missing.store(true, Ordering::Relaxed);
                        block = Some(BlockData {
                            height,
                            receipts,
                            events,
                            ..Default::default()
                        });
                    }
                }
            }
            None => {
                if block.is_none() {
                    return Ok(None);
                }
            }
        }
    }
    let Some(mut b) = block else { return Ok(None) };
    if b.actions.is_none() && probe.should_try(&probe.actions_missing, height) {
        match node.get(&format!("{path}/actions")).await? {
            Some(v) => {
                probe.actions_missing.store(false, Ordering::Relaxed);
                b.actions = Some(parse_actions(&v));
            }
            None => probe.actions_missing.store(true, Ordering::Relaxed),
        }
    }
    Ok(Some(b))
}

/// Actions for a block that arrived over the WebSocket (which carries none).
pub async fn fetch_actions(
    node: &dyn NodeClient,
    probe: &RouteProbe,
    b: &mut BlockData,
) -> Result<()> {
    if b.actions.is_some() || !probe.should_try(&probe.actions_missing, b.height) {
        return Ok(());
    }
    match node
        .get(&format!("/v1/blocks/{}/actions", b.height))
        .await?
    {
        Some(v) => {
            probe.actions_missing.store(false, Ordering::Relaxed);
            b.actions = Some(parse_actions(&v));
        }
        None => probe.actions_missing.store(true, Ordering::Relaxed),
    }
    Ok(())
}

// ---------------- HTTP client ----------------

pub struct HttpNodeClient {
    http: reqwest::Client,
    rpc: String,
    ws: String,
}

impl HttpNodeClient {
    pub fn new(rpc: &str, ws: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .pool_max_idle_per_host(32)
            .build()?;
        Ok(Self {
            http,
            rpc: rpc.trim_end_matches('/').to_string(),
            ws: ws.to_string(),
        })
    }
}

#[async_trait]
impl NodeClient for HttpNodeClient {
    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", self.rpc, path);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        let json: Value = serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
        if !status.is_success() {
            return Err(anyhow!("POST {url}: HTTP {status}: {json}"));
        }
        Ok(json)
    }

    async fn get(&self, path: &str) -> Result<Option<Value>> {
        let url = format!("{}{}", self.rpc, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(anyhow!("GET {url}: HTTP {}", resp.status()));
        }
        let text = resp.text().await?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(
            serde_json::from_str(&text).with_context(|| format!("GET {url}: bad JSON"))?,
        ))
    }

    async fn subscribe(&self) -> Result<BoxStream<'static, BlockData>> {
        let (socket, _) = tokio_tungstenite::connect_async(&self.ws)
            .await
            .with_context(|| format!("connect {}", self.ws))?;
        let (_, read) = socket.split();
        let stream = read
            .take_while(|m| futures::future::ready(m.is_ok()))
            .filter_map(|m| async move {
                match m {
                    Ok(tokio_tungstenite::tungstenite::Message::Text(t)) => {
                        serde_json::from_str::<Value>(&t)
                            .ok()
                            .and_then(|v| parse_block_update(&v))
                    }
                    _ => None,
                }
            });
        Ok(stream.boxed())
    }
}

// ---------------- mock (tests, fixtures) ----------------

/// In-memory node: blocks by height, a route table for snapshot paths, and
/// a push channel standing in for `/v1/ws`. `serve_blocks_route = false`
/// mimics a node without `/v1/blocks/{h}`; `serve_actions = false` one
/// without `/v1/blocks/{h}/actions`.
pub struct MockNodeClient {
    pub blocks: Mutex<BTreeMap<u64, BlockData>>,
    pub routes: Mutex<HashMap<String, Value>>,
    pub serve_blocks_route: AtomicBool,
    pub serve_actions: AtomicBool,
    pub chain_id: u64,
    tx: tokio::sync::broadcast::Sender<BlockData>,
    pub calls: Mutex<Vec<String>>,
}

impl Default for MockNodeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MockNodeClient {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(1024);
        Self {
            blocks: Mutex::new(BTreeMap::new()),
            routes: Mutex::new(HashMap::new()),
            serve_blocks_route: AtomicBool::new(true),
            serve_actions: AtomicBool::new(true),
            chain_id: 1,
            tx,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn add_block(&self, b: BlockData) {
        self.blocks.lock().expect("lock").insert(b.height, b);
    }

    pub fn set_route(&self, path: &str, v: Value) {
        self.routes
            .lock()
            .expect("lock")
            .insert(path.to_string(), v);
    }

    /// Push a block to WebSocket subscribers (also stored for HTTP).
    pub fn push_block(&self, b: BlockData) {
        self.add_block(b.clone());
        let _ = self.tx.send(b);
    }

    pub fn tip(&self) -> Option<BlockData> {
        self.blocks.lock().expect("lock").values().last().cloned()
    }

    fn status_json(&self) -> Value {
        let tip = self.tip();
        serde_json::json!({
            "chain_id": self.chain_id,
            "height": tip.as_ref().map(|b| b.height).unwrap_or(0),
            "timestamp": tip.as_ref().and_then(|b| b.timestamp).unwrap_or(0),
            "state_hash": tip.as_ref().and_then(|b| b.state_hash.clone()).unwrap_or_default(),
            "validators": [],
            "mempool": 0,
            "accounts": 0,
            "orders": 0,
        })
    }

    fn block_json(b: &BlockData, with_receipts: bool) -> Value {
        let mut v = serde_json::json!({ "height": b.height, "timestamp": b.timestamp, "state_hash": b.state_hash, "tx_count": b.receipts.len() });
        if with_receipts {
            v["receipts"] = Value::Array(b.receipts.iter().map(receipt_json).collect());
            v["events"] = Value::Array(b.events.clone());
        }
        v
    }
}

pub fn receipt_json(r: &crate::types::ReceiptRow) -> Value {
    serde_json::json!({
        "index": r.index, "tx_id": r.tx_id, "signer": r.signer, "ok": r.ok,
        "error": r.error.as_ref().map(|(c, m)| serde_json::json!({"code": c, "message": m})),
        "events": r.events, "timestamp": r.timestamp,
    })
}

#[async_trait]
impl NodeClient for MockNodeClient {
    async fn get(&self, path: &str) -> Result<Option<Value>> {
        self.calls.lock().expect("lock").push(path.to_string());
        if path == "/v1/status" {
            return Ok(Some(self.status_json()));
        }
        if let Some(rest) = path.strip_prefix("/v1/blocks/") {
            let mut parts = rest.splitn(2, '/');
            let h: u64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let sub = parts.next().unwrap_or("");
            let blocks = self.blocks.lock().expect("lock");
            let Some(b) = blocks.get(&h) else {
                return Ok(None);
            };
            return Ok(match sub {
                "" => self.serve_blocks_route.load(Ordering::Relaxed).then(|| Self::block_json(b, false)),
                "receipts" => Some(serde_json::json!({ "height": h, "receipts": b.receipts.iter().map(receipt_json).collect::<Vec<_>>(), "events": b.events })),
                "actions" => (self.serve_actions.load(Ordering::Relaxed) && b.actions.is_some()).then(|| {
                    serde_json::json!({
                        "height": h,
                        "actions": b.actions.iter().flatten().map(|a| serde_json::json!({"index": a.index, "tx_id": a.tx_id, "signer": a.signer, "nonce": a.nonce, "action": a.action})).collect::<Vec<_>>()
                    })
                }),
                _ => None,
            });
        }
        let routes = self.routes.lock().expect("lock");
        if let Some(v) = routes.get(path) {
            return Ok(Some(v.clone()));
        }
        // Query strings: fall back to the bare path.
        let bare = path.split('?').next().unwrap_or(path);
        Ok(routes.get(bare).cloned())
    }

    async fn subscribe(&self) -> Result<BoxStream<'static, BlockData>> {
        let rx = self.tx.subscribe();
        let stream = futures::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(b) => return Some((b, rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        });
        Ok(stream.boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ReceiptRow;

    fn block(h: u64) -> BlockData {
        BlockData {
            height: h,
            timestamp: Some(1_000 * h),
            state_hash: Some(format!("{h:064x}")),
            receipts: vec![ReceiptRow {
                index: 0,
                tx_id: format!("{h:064x}"),
                signer: "aa".repeat(32),
                ok: true,
                error: None,
                events: vec![],
                timestamp: Some(1_000 * h),
            }],
            actions: Some(vec![]),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn fetch_block_falls_back_to_receipts_route() {
        let mock = MockNodeClient::new();
        mock.add_block(block(3));
        let probe = RouteProbe::default();
        let b = fetch_block(&mock, &probe, 3).await.unwrap().unwrap();
        assert_eq!(b.timestamp, Some(3_000));
        assert!(b.actions.is_some());
        assert!(fetch_block(&mock, &probe, 4).await.unwrap().is_none());

        mock.serve_blocks_route.store(false, Ordering::Relaxed);
        mock.serve_actions.store(false, Ordering::Relaxed);
        let probe = RouteProbe::default();
        let b = fetch_block(&mock, &probe, 3).await.unwrap().unwrap();
        assert_eq!(b.timestamp, None, "header route missing: no timestamp");
        assert_eq!(b.best_timestamp(), Some(3_000), "receipt timestamp used");
        assert!(b.actions.is_none());
        assert!(probe.blocks_route_missing() && probe.actions_route_missing());
        // The next height does not retry the missing routes.
        mock.calls.lock().unwrap().clear();
        mock.add_block(block(4));
        let _ = fetch_block(&mock, &probe, 4).await.unwrap();
        let calls = mock.calls.lock().unwrap().clone();
        assert_eq!(calls, vec!["/v1/blocks/4/receipts".to_string()]);
    }
}
