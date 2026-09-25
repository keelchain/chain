//! HTTP + WebSocket API. The node implements [`NodeApi`]; this crate only
//! reads state and forwards signed actions. See API.md.
//!
//! This crate sits above the VM: tokio, HashMap and wall clock are fine
//! here (dev-rules.md applies to `keel-vm` and below).
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State as AxumState,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use keel_actions::SignedAction;
use keel_types::{Address, OrderId, Side};
use keel_vm::{receipt::Receipt, Event, State, VmError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

/// One applied block, streamed to WebSocket subscribers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockUpdate {
    pub height: u64,
    pub timestamp: u64,
    pub state_hash: String,
    pub receipts: Vec<Receipt>,
    pub events: Vec<Event>,
}

/// One block's summary as the archive keeps it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockMeta {
    pub height: u64,
    pub timestamp: u64,
    pub state_hash: Option<String>,
    pub tx_count: u32,
}

/// What the RPC needs from the node.
pub trait NodeApi: Send + Sync + 'static {
    fn state(&self) -> Arc<Mutex<State>>;
    /// Admit into the mempool (and gossip). Returns the tx id.
    fn submit(&self, action: SignedAction) -> Result<[u8; 32], VmError>;
    fn mempool_len(&self) -> usize;
    fn receipt(&self, tx_id: &[u8; 32]) -> Option<Receipt>;
    fn receipts_at(&self, height: u64) -> Option<Vec<Receipt>>;
    /// End-of-block events of `height` (batches, releases, invariants).
    fn events_at(&self, height: u64) -> Option<Vec<Event>> {
        let _ = height;
        None
    }
    /// Block metadata from durable history (any height). `None` = unknown height.
    fn block_meta(&self, height: u64) -> Option<BlockMeta> {
        let _ = height;
        None
    }
    /// Newest-first page of block metadata before `before` (exclusive).
    fn block_metas(&self, before: Option<u64>, limit: usize) -> Vec<BlockMeta> {
        let _ = (before, limit);
        Vec::new()
    }
    /// The signed actions of a block from durable history.
    fn block_actions(&self, height: u64) -> Option<Vec<SignedAction>> {
        let _ = height;
        None
    }
    /// Receipts from durable history when the in-memory window has moved on.
    fn receipts_archived(&self, height: u64) -> Option<Vec<Receipt>> {
        let _ = height;
        None
    }
    /// Which external network the vault addresses are encoded for
    /// (mainnet on a real chain, regtest on a devnet).
    fn external_network(&self) -> keel_chains::Network {
        keel_chains::Network::Mainnet
    }
    fn subscribe(&self) -> broadcast::Receiver<BlockUpdate>;
    fn validators(&self) -> Vec<String>;
}

type Node = Arc<dyn NodeApi>;

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
    code: &'static str,
}

fn err(
    status: StatusCode,
    code: &'static str,
    msg: impl Into<String>,
) -> (StatusCode, Json<ErrorBody>) {
    (
        status,
        Json(ErrorBody {
            error: msg.into(),
            code,
        }),
    )
}

fn not_found(what: &str) -> (StatusCode, Json<ErrorBody>) {
    err(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        format!("{what} not found"),
    )
}

fn parse_address(s: &str) -> Result<Address, (StatusCode, Json<ErrorBody>)> {
    Address::from_hex(s).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "BAD_ADDRESS",
            "address must be 64 hex chars",
        )
    })
}

fn to_json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

pub fn router(node: Node) -> Router {
    Router::new()
        .route("/v1/actions", post(submit_action))
        .route("/v1/status", get(status))
        .route("/v1/params", get(params))
        .route("/v1/lightning", get(lightning))
        .route("/v1/accounts/{addr}", get(account))
        .route("/v1/accounts/{addr}/orders", get(account_orders))
        .route("/v1/accounts/{addr}/trades", get(account_trades))
        .route("/v1/markets", get(markets))
        .route("/v1/markets/{pair}", get(market))
        .route("/v1/markets/{pair}/book", get(book))
        .route("/v1/orders/{id}", get(order))
        .route("/v1/offers", get(offers))
        .route("/v1/offers/{id}", get(offer))
        .route("/v1/trades/{id}", get(trade))
        .route("/v1/vaults/outbounds", get(outbounds))
        .route("/v1/vaults/deposits", get(deposits))
        .route(
            "/v1/vaults/{chain}/addresses/lookup",
            get(vault_address_lookup),
        )
        .route("/v1/vaults/{chain}/addresses/{index}", get(vault_address))
        .route("/v1/vaults/{chain}", get(vault))
        .route("/v1/vaults/{chain}/addresses", get(vault_addresses))
        .route("/v1/gov/proposals", get(proposals))
        .route("/v1/gov/roles", get(roles))
        .route("/v1/gov/proposals/{id}", get(proposal))
        .route("/v1/staking/validators", get(validators))
        .route("/v1/receipts/{tx_id}", get(receipt))
        .route("/v1/blocks", get(blocks))
        .route("/v1/blocks/{height}", get(block))
        .route("/v1/blocks/{height}/actions", get(block_actions))
        .route("/v1/blocks/{height}/receipts", get(block_receipts))
        .route("/v1/ws", get(ws))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(node)
}

/// Serve until the listener fails. Meant to be spawned by the node.
pub async fn serve(node: Node, addr: SocketAddr) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "rpc listening");
    axum::serve(listener, router(node)).await
}

// ---------------- handlers ----------------

async fn submit_action(
    AxumState(node): AxumState<Node>,
    Json(sa): Json<SignedAction>,
) -> impl IntoResponse {
    match node.submit(sa) {
        Ok(id) => (
            StatusCode::ACCEPTED,
            Json(json!({ "tx_id": hex::encode(id), "admitted": true })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "admitted": false, "error": e.to_string(), "code": e.code() })),
        )
            .into_response(),
    }
}

async fn status(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    Json(json!({
        "chain_id": s.chain_id,
        "height": s.height,
        "timestamp": s.timestamp,
        "state_hash": hex::encode(s.last_hash),
        "validators": node.validators(),
        "mempool": node.mempool_len(),
        "accounts": s.accounts.len(),
        "orders": s.markets.orders.len(),
    }))
}

async fn params(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    Json(to_json(&s.params))
}

#[derive(Serialize)]
struct BalanceRow {
    asset: String,
    account_type: String,
    balance: String,
}

async fn account(
    AxumState(node): AxumState<Node>,
    Path(addr): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let address = parse_address(&addr)?;
    let state = node.state();
    let s = state.lock().expect("state lock");
    let meta = s.account_ref(&address).cloned().unwrap_or_default();
    let p = &s.params.budget;
    let balances: Vec<BalanceRow> = s
        .ledger
        .accounts_of(address)
        .map(|(k, st)| BalanceRow {
            asset: k.asset.to_string(),
            account_type: k.account_type.to_string(),
            balance: st.balance.to_string(),
        })
        .collect();
    let tier = if meta.tier_expires_at > s.timestamp / 1000 {
        meta.tier
    } else {
        0
    };
    // Lock pool as it would be at the current block time (regen is lazy).
    let lock_pool = {
        let mut b = meta.budget.clone();
        b.regen(p, s.timestamp);
        b.lock_pool
    };
    let sessions: Vec<Value> = keel_vm::modules::sessions::sessions_of(&s, address, s.timestamp / 1000)
        .iter()
        .map(|(k, sess)| json!({ "key": k.to_hex(), "scope": sess.scope, "expires_at": sess.expires_at }))
        .collect();
    let session_of: Option<Value> = s.sessions.by_key.get(&address).map(|sess| json!({ "principal": sess.principal.to_hex(), "scope": sess.scope, "expires_at": sess.expires_at }));
    let unlocking: Vec<Value> = keel_vm::modules::budgets::pending_unlocks(&s, address)
        .iter()
        .map(|(t, a)| json!({ "ready_at": t, "amount": a.to_string() }))
        .collect();
    Ok(Json(json!({
        "address": addr,
        "nonce": meta.nonce,
        "sessions": sessions,
        "session_of": session_of,
        "budget": {
            "used": meta.budget.used,
            "limit": meta.budget.limit(p),
            "cancel_limit": meta.budget.cancel_limit(p),
            "remaining": meta.budget.remaining(p),
            "earned": meta.budget.earned,
            "locked": meta.budget.locked.to_string(),
            "lock_cap_per_day": meta.budget.lock_cap(p),
            "lock_pool": lock_pool,
            "unlocking": unlocking,
        },
        "tier": tier,
        "tier_expires_at": meta.tier_expires_at,
        "balances": balances,
    })))
}

async fn account_orders(
    AxumState(node): AxumState<Node>,
    Path(addr): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let address = parse_address(&addr)?;
    let state = node.state();
    let s = state.lock().expect("state lock");
    let open: Vec<Value> = s
        .markets
        .open_orders_of(&address)
        .into_iter()
        .map(to_json)
        .collect();
    Ok(Json(json!({ "open": open })))
}

async fn account_trades(
    AxumState(node): AxumState<Node>,
    Path(addr): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let _ = parse_address(&addr)?;
    let state = node.state();
    let s = state.lock().expect("state lock");
    let p2p = to_json(&s.p2p);
    let trades = collection(&p2p, "trades")
        .into_iter()
        .filter(|t| field_str(t, "buyer") == Some(&addr) || field_str(t, "seller") == Some(&addr))
        .collect::<Vec<_>>();
    Ok(Json(json!({ "trades": trades })))
}

async fn markets(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let list: Vec<Value> = s.markets.pairs.values().map(market_summary).collect();
    Json(json!({ "markets": list }))
}

fn market_summary(m: &keel_vm::modules::markets::Market) -> Value {
    json!({
        "symbol": m.cfg.symbol,
        "cfg": to_json(&m.cfg),
        "house": to_json(&m.house),
        "last_price": m.last_price.map(|p| p.to_string()),
        "best_bid": m.book.best_price(Side::Buy).map(|p| p.to_string()),
        "best_ask": m.book.best_price(Side::Sell).map(|p| p.to_string()),
        "volume_base": m.volume_base.to_string(),
        "volume_quote": m.volume_quote.to_string(),
        "open_orders": m.book.order_count(),
    })
}

async fn market(
    AxumState(node): AxumState<Node>,
    Path(pair): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let m = s
        .markets
        .pairs
        .get(&pair)
        .ok_or_else(|| not_found("pair"))?;
    Ok(Json(market_summary(m)))
}

#[derive(Deserialize)]
struct DepthQuery {
    depth: Option<usize>,
}

async fn book(
    AxumState(node): AxumState<Node>,
    Path(pair): Path<String>,
    Query(q): Query<DepthQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let depth = q.depth.unwrap_or(20).clamp(1, 200);
    let state = node.state();
    let s = state.lock().expect("state lock");
    let m = s
        .markets
        .pairs
        .get(&pair)
        .ok_or_else(|| not_found("pair"))?;
    let levels = |side: Side| -> Vec<Value> {
        m.book
            .levels(side, depth)
            .into_iter()
            .map(|l| json!({ "price": l.price.to_string(), "size": l.size.to_string(), "orders": l.orders }))
            .collect()
    };
    Ok(Json(json!({
        "symbol": pair,
        "height": s.height,
        "bids": levels(Side::Buy),
        "asks": levels(Side::Sell),
        "house": to_json(&m.house),
    })))
}

async fn order(
    AxumState(node): AxumState<Node>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let o = s
        .markets
        .orders
        .get(&OrderId(id))
        .ok_or_else(|| not_found("order"))?;
    Ok(Json(to_json(o)))
}

// ---- generic sub-state navigation (other modules evolve independently) ----

/// Values of a map-like field (`{"1": {...}, "2": {...}}` or a list).
fn collection(root: &Value, key: &str) -> Vec<Value> {
    match root.get(key) {
        Some(Value::Object(m)) => m.values().cloned().collect(),
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn field_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn item_by_id(root: &Value, key: &str, id: u64) -> Option<Value> {
    match root.get(key) {
        Some(Value::Object(m)) => m.get(&id.to_string()).cloned(),
        Some(Value::Array(a)) => a
            .iter()
            .find(|x| x.get("id").and_then(Value::as_u64) == Some(id))
            .cloned(),
        _ => None,
    }
}

#[derive(Deserialize)]
struct OfferQuery {
    asset: Option<String>,
    side: Option<String>,
    fiat: Option<String>,
}

async fn offers(AxumState(node): AxumState<Node>, Query(q): Query<OfferQuery>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let p2p = to_json(&s.p2p);
    let list: Vec<Value> = collection(&p2p, "offers")
        .into_iter()
        .filter(|o| {
            let spec = o.get("spec").cloned().unwrap_or(Value::Null);
            let closed = o.get("closed").and_then(Value::as_bool).unwrap_or(false);
            !closed
                && q.asset
                    .as_deref()
                    .is_none_or(|a| field_str(&spec, "asset") == Some(a))
                && q.side
                    .as_deref()
                    .is_none_or(|sd| field_str(&spec, "side") == Some(sd))
                && q.fiat
                    .as_deref()
                    .is_none_or(|f| field_str(&spec, "fiat_currency") == Some(f))
        })
        .collect();
    Json(json!({ "offers": list }))
}

async fn offer(
    AxumState(node): AxumState<Node>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let p2p = to_json(&s.p2p);
    item_by_id(&p2p, "offers", id)
        .map(Json)
        .ok_or_else(|| not_found("offer"))
}

async fn trade(
    AxumState(node): AxumState<Node>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let p2p = to_json(&s.p2p);
    let mut t = item_by_id(&p2p, "trades", id).ok_or_else(|| not_found("trade"))?;
    let disputes = to_json(&s.disputes);
    if let Some(d) = item_by_id(&disputes, "disputes", id) {
        t["dispute"] = d;
    }
    Ok(Json(t))
}

fn vault_json(v: &keel_vm::modules::vaults::Vault) -> Value {
    json!({
        "chain": v.chain.as_str(),
        "epoch": v.epoch,
        "public_key": hex::encode(&v.public_key),
        "chain_code": v.chain_code.map(hex::encode),
        "signers": v.signers.iter().map(Address::to_hex).collect::<Vec<_>>(),
        "threshold": v.threshold,
        "registered_height": v.registered_height,
    })
}

fn parse_chain(chain: &str) -> Result<keel_actions::Chain, (StatusCode, Json<ErrorBody>)> {
    keel_actions::Chain::parse(chain)
        .or(match chain {
            "Bitcoin" => Some(keel_actions::Chain::Bitcoin),
            "Ethereum" => Some(keel_actions::Chain::Ethereum),
            "Tron" => Some(keel_actions::Chain::Tron),
            _ => None,
        })
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "INVALID",
                "unknown chain (BTC, ETH, TRON)",
            )
        })
}

/// The active vault of a chain, its deposit-index owner map and the
/// observers' fee median. Typed view: the vault state has tuple-keyed
/// maps that generic JSON cannot express.
async fn vault(
    AxumState(node): AxumState<Node>,
    Path(chain): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let chain = parse_chain(&chain)?;
    let state = node.state();
    let s = state.lock().expect("state lock");
    let v = &s.vaults;
    let owners: serde_json::Map<String, Value> = v
        .deposit_owner
        .range((chain, 0)..=(chain, u64::MAX))
        .map(|((_, i), a)| (i.to_string(), Value::String(a.to_hex())))
        .collect();
    let epochs: Vec<Value> = v
        .vaults
        .range((chain, 0)..=(chain, u64::MAX))
        .map(|(_, x)| vault_json(x))
        .collect();
    Ok(Json(json!({
        "chain": chain.as_str(),
        "vault": v.active_vault(chain).map(vault_json),
        "epochs": epochs,
        "next_deposit_index": v.next_deposit_index.get(&chain).copied().unwrap_or(1),
        "deposit_owners": owners,
        "fee_rate": v.fee_rate(chain),
        "halted": v.halted.iter().filter(|a| a.chain() == Some(chain.as_str())).map(|a| a.to_string()).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct FromQuery {
    from: Option<u64>,
    limit: Option<usize>,
    /// Only this account's deposit indexes (64 hex).
    owner: Option<String>,
}

/// Deposit indexes of a chain from `from`, with their owners.
async fn vault_addresses(
    AxumState(node): AxumState<Node>,
    Path(chain): Path<String>,
    Query(q): Query<FromQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let chain = parse_chain(&chain)?;
    let network = node.external_network();
    let state = node.state();
    let s = state.lock().expect("state lock");
    let from = q.from.unwrap_or(0);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let owner = q.owner.as_deref().map(parse_address).transpose()?;
    let rows: Vec<Value> = s
        .vaults
        .deposit_owner
        .range((chain, from)..=(chain, u64::MAX))
        .filter(|(_, a)| owner.is_none_or(|o| **a == o))
        .take(limit)
        .map(|((_, i), a)| json!({ "index": i, "owner": a.to_hex(), "address": derive(&s.vaults, chain, network, *i) }))
        .collect();
    Ok(Json(json!({
        "chain": chain.as_str(),
        "next_deposit_index": s.vaults.next_deposit_index.get(&chain).copied().unwrap_or(1),
        "addresses": rows,
    })))
}

/// The deposit address string of `index` under the active vault, or null
/// when no vault is registered for the chain.
fn derive(
    v: &keel_vm::modules::vaults::VaultsState,
    chain: keel_actions::Chain,
    network: keel_chains::Network,
    index: u64,
) -> Option<String> {
    let vault = v.active_vault(chain)?;
    let pk: [u8; 33] = vault.public_key.as_slice().try_into().ok()?;
    let cc = vault.chain_code?;
    keel_chains::deposit_address(chain, network, &pk, &cc, index).ok()
}

/// One assigned deposit index: owner and derived address.
async fn vault_address(
    AxumState(node): AxumState<Node>,
    Path((chain, index)): Path<(String, u64)>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let chain = parse_chain(&chain)?;
    let network = node.external_network();
    let state = node.state();
    let s = state.lock().expect("state lock");
    let owner = s
        .vaults
        .deposit_owner
        .get(&(chain, index))
        .ok_or_else(|| not_found("deposit index"))?;
    let address = derive(&s.vaults, chain, network, index).ok_or_else(|| {
        err(
            StatusCode::CONFLICT,
            "NO_VAULT",
            "no vault registered for this chain yet",
        )
    })?;
    Ok(Json(
        json!({ "chain": chain.as_str(), "index": index, "owner": owner.to_hex(), "address": address }),
    ))
}

#[derive(Deserialize)]
struct LookupQuery {
    address: String,
}

/// Reverse lookup: which assigned index encodes to `address`.
async fn vault_address_lookup(
    AxumState(node): AxumState<Node>,
    Path(chain): Path<String>,
    Query(q): Query<LookupQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let chain = parse_chain(&chain)?;
    let network = node.external_network();
    let state = node.state();
    let s = state.lock().expect("state lock");
    let wanted = q.address.trim();
    let hit = s
        .vaults
        .deposit_owner
        .range((chain, 0)..=(chain, u64::MAX))
        .find_map(|((_, i), owner)| {
            let a = derive(&s.vaults, chain, network, *i)?;
            (a.eq_ignore_ascii_case(wanted)).then(|| json!({ "chain": chain.as_str(), "index": i, "owner": owner.to_hex(), "address": a }))
        });
    hit.map(Json).ok_or_else(|| not_found("deposit address"))
}

#[derive(Deserialize)]
struct DepositsQuery {
    status: Option<String>,
    owner: Option<String>,
}

/// Pending, credited (held) and rejected deposits as the chain knows them.
async fn deposits(
    AxumState(node): AxumState<Node>,
    Query(q): Query<DepositsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let network = node.external_network();
    let owner_filter = match &q.owner {
        Some(h) => Some(Address::from_hex(h).ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "INVALID",
                "owner must be 64 hex chars",
            )
        })?),
        None => None,
    };
    let state = node.state();
    let s = state.lock().expect("state lock");
    let v = &s.vaults;
    let mut rows = Vec::new();
    for (digest, p) in &v.pending {
        let a = &p.attestation;
        let owner = v.deposit_owner.get(&(a.chain, a.deposit_index)).copied();
        if owner_filter.is_some() && owner != owner_filter {
            continue;
        }
        let status = format!("{:?}", p.status);
        if q.status
            .as_deref()
            .is_some_and(|st| !status.eq_ignore_ascii_case(st))
        {
            continue;
        }
        rows.push(json!({
            "chain": a.chain.as_str(),
            "asset": a.asset.to_string(),
            "tx_hash": hex::encode(a.tx_hash),
            "index": a.index,
            "deposit_index": a.deposit_index,
            "owner": owner.map(|o| o.to_hex()),
            "address": derive(v, a.chain, network, a.deposit_index),
            "amount": a.amount.to_string(),
            "external_height": a.external_height,
            "votes": p.quorum.count(),
            "status": status,
            "first_height": p.first_height,
            "last_height": p.last_height,
            "digest": hex::encode(digest),
        }));
    }
    for h in &v.held {
        if owner_filter.is_some_and(|o| o != h.owner) {
            continue;
        }
        if q.status
            .as_deref()
            .is_some_and(|st| !st.eq_ignore_ascii_case("held"))
        {
            continue;
        }
        rows.push(json!({
            "asset": h.asset.to_string(),
            "owner": h.owner.to_hex(),
            "amount": h.amount.to_string(),
            "status": "Held",
            "release_height": h.release_height,
            "external_id": h.external_id,
        }));
    }
    Ok(Json(json!({ "deposits": rows })))
}

#[derive(Deserialize)]
struct StatusQuery {
    status: Option<String>,
}

fn outbound_json(o: &keel_vm::modules::vaults::Outbound) -> Value {
    json!({
        "id": o.id,
        "owner": o.owner.to_hex(),
        "asset": o.asset.to_string(),
        "chain": o.chain.as_str(),
        "to": o.to,
        "amount": o.amount.to_string(),
        "fee_asset": o.fee_asset.to_string(),
        "fee_estimate": o.fee_estimate.to_string(),
        "status": format!("{:?}", o.status),
        "batch_id": o.batch_id,
        "created_height": o.created_height,
        "tx_hash": o.tx_hash.map(hex::encode),
        "votes": o.quorum.count(),
    })
}

/// Outbound withdrawals, optionally filtered by status
/// (`Queued|Batched|Confirmed|Failed`), plus the batches they belong to.
/// Lightning pools per observer and the limits in force (2026-09-10).
async fn lightning(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let p = &s.params;
    let pools: Vec<Value> = keel_vm::modules::lightning::pools_view(&s)
        .iter()
        .map(|(a, pool)| {
            json!({
                "observer": a.to_hex(),
                "node_id": hex::encode(&pool.node_id),
                "balance": pool.balance.to_string(),
                "pending_out": pool.pending_out.to_string(),
                "available": pool.available().to_string(),
                "credited_today": pool.credited_today.1.to_string(),
                "registered_height": pool.registered_height,
            })
        })
        .collect();
    let assignments: Vec<Value> = s
        .lightning
        .assignments
        .iter()
        .map(|(id, a)| {
            let ob = s.vaults.outbounds.get(id);
            json!({
                "outbound_id": id,
                "observer": a.observer.to_hex(),
                "deadline_height": a.deadline_height,
                "fee_allowance": a.fee_allowance.to_string(),
                "invoice": ob.map(|o| o.to.clone()),
                "amount": ob.map(|o| o.amount.to_string()),
                "owner": ob.map(|o| o.owner.to_hex()),
            })
        })
        .collect();
    let sweeps: Vec<Value> = s.lightning.sweeps.iter().map(|(h, (o, amt))| json!({ "tx_hash": hex::encode(h), "observer": o.to_hex(), "amount": amt.to_string() })).collect();
    Json(json!({
        "enabled": p.lightning_enabled != 0,
        "params": {
            "pool_cap_sats": p.lightning_pool_cap_sats,
            "max_deposit_sats": p.lightning_max_deposit_sats,
            "max_withdraw_sats": p.lightning_max_withdraw_sats,
            "max_fee_bps": p.lightning_max_fee_bps,
            "min_fee_sats": p.lightning_min_fee_sats,
            "payout_timeout_blocks": p.lightning_payout_timeout_blocks,
            "daily_cap_sats": p.lightning_daily_cap_sats,
        },
        "pools": pools,
        "assignments": assignments,
        "sweeps": sweeps,
        "pool_total": s.ledger.balance(&keel_ledger::AccountKey::new(keel_types::Address::SYSTEM, keel_vm::modules::vaults::native_asset(keel_actions::Chain::Bitcoin), "lightning_pool").expect("catalog")).to_string(),
    }))
}

async fn outbounds(AxumState(node): AxumState<Node>, Query(q): Query<StatusQuery>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let list: Vec<Value> = s
        .vaults
        .outbounds
        .values()
        .filter(|o| {
            q.status
                .as_deref()
                .is_none_or(|st| format!("{:?}", o.status).eq_ignore_ascii_case(st))
        })
        .map(outbound_json)
        .collect();
    let batches: Vec<Value> = s
        .vaults
        .batches
        .values()
        .map(|b| json!({ "id": b.id, "chain": b.chain.as_str(), "outbound_ids": b.outbound_ids, "created_height": b.created_height }))
        .collect();
    Json(json!({ "outbounds": list, "batches": batches }))
}

async fn proposals(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let gov = to_json(&s.gov);
    Json(
        json!({ "proposals": collection(&gov, "proposals"), "house_operator": s.gov_house_operator.map(|a| a.to_hex()) }),
    )
}

/// Governance-elected role sets: who attests, observes, arbitrates, and
/// who may `SetParam` directly. Clients onboarding (see
/// infra/testnet/onboard-client.sh) read the current members here before
/// proposing a replacement set.
async fn roles(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let hex = |set: &std::collections::BTreeSet<Address>| {
        set.iter().map(|a| a.to_hex()).collect::<Vec<_>>()
    };
    Json(json!({
        "attesters": hex(&s.attest.attesters),
        "observers": hex(&s.staking.observers),
        "observer_threshold": s.staking.observer_threshold,
        "arbitrators": hex(&s.staking.arbitrators),
        "param_admin": s.gov.param_admin.map(|a| a.to_hex()),
        "house_operator": s.gov_house_operator.map(|a| a.to_hex()),
    }))
}

async fn proposal(
    AxumState(node): AxumState<Node>,
    Path(id): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let gov = to_json(&s.gov);
    item_by_id(&gov, "proposals", id)
        .map(Json)
        .ok_or_else(|| not_found("proposal"))
}

async fn validators(AxumState(node): AxumState<Node>) -> Json<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let set: Vec<Value> = keel_vm::modules::staking::validator_set(&s)
        .into_iter()
        .map(
            |(key, power)| json!({ "consensus_key": hex::encode(key), "power": power.to_string() }),
        )
        .collect();
    Json(json!({ "consensus": node.validators(), "staked": set, "staking": to_json(&s.staking) }))
}

async fn receipt(
    AxumState(node): AxumState<Node>,
    Path(tx_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let bytes = hex::decode(&tx_id)
        .ok()
        .and_then(|v| <[u8; 32]>::try_from(v).ok())
        .ok_or_else(|| {
            err(
                StatusCode::BAD_REQUEST,
                "BAD_TX_ID",
                "tx id must be 64 hex chars",
            )
        })?;
    node.receipt(&bytes)
        .map(|r| Json(receipt_json(&r)))
        .ok_or_else(|| not_found("receipt"))
}

fn receipt_json(r: &Receipt) -> Value {
    json!({
        "index": r.index,
        "height": r.height,
        "timestamp": r.timestamp,
        "tx_id": hex::encode(r.tx_id),
        "signer": r.signer.to_hex(),
        "ok": r.ok,
        "error": r.error.as_ref().map(|e| json!({ "code": e.code(), "message": e.to_string() })),
        "events": to_json(&r.events),
    })
}

#[derive(Deserialize)]
struct BlocksQuery {
    before: Option<u64>,
    limit: Option<usize>,
}

/// Newest-first block list from durable history.
async fn blocks(AxumState(node): AxumState<Node>, Query(q): Query<BlocksQuery>) -> Json<Value> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let list = node.block_metas(q.before, limit);
    let next = list.last().map(|b| b.height);
    Json(json!({ "blocks": list, "next_before": next }))
}

async fn block(
    AxumState(node): AxumState<Node>,
    Path(height): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let meta = node.block_meta(height).ok_or_else(|| not_found("block"))?;
    Ok(Json(serde_json::to_value(meta).unwrap_or(Value::Null)))
}

/// The block's signed actions, decoded, with their tx ids.
async fn block_actions(
    AxumState(node): AxumState<Node>,
    Path(height): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let actions = node
        .block_actions(height)
        .ok_or_else(|| not_found("block"))?;
    let list: Vec<Value> = actions
        .iter()
        .enumerate()
        .map(|(i, sa)| {
            json!({
                "index": i,
                "tx_id": hex::encode(sa.id()),
                "signer": sa.envelope.signer.to_hex(),
                "nonce": sa.envelope.nonce,
                "action": to_json(&sa.envelope.action),
            })
        })
        .collect();
    Ok(Json(json!({ "height": height, "actions": list })))
}

async fn block_receipts(
    AxumState(node): AxumState<Node>,
    Path(height): Path<u64>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorBody>)> {
    let list = node
        .receipts_at(height)
        .or_else(|| node.receipts_archived(height))
        .ok_or_else(|| not_found("block"))?;
    let events = node.events_at(height).unwrap_or_default();
    Ok(Json(
        json!({ "height": height, "receipts": list.iter().map(receipt_json).collect::<Vec<_>>(), "events": to_json(&events) }),
    ))
}

async fn ws(ws: WebSocketUpgrade, AxumState(node): AxumState<Node>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_blocks(socket, node))
}

async fn stream_blocks(mut socket: WebSocket, node: Node) {
    let mut rx = node.subscribe();
    loop {
        match rx.recv().await {
            Ok(update) => {
                let mut v = to_json(&update);
                if let Some(receipts) = v.get_mut("receipts") {
                    *receipts = Value::Array(update.receipts.iter().map(receipt_json).collect());
                }
                // Book snapshot after each block, for market UIs.
                let books: BTreeMap<String, Value> = {
                    let state = node.state();
                    let s = state.lock().expect("state lock");
                    s.markets
                        .pairs
                        .iter()
                        .map(|(k, m)| {
                            (k.clone(), json!({
                                "best_bid": m.book.best_price(Side::Buy).map(|p| p.to_string()),
                                "best_ask": m.book.best_price(Side::Sell).map(|p| p.to_string()),
                                "last_price": m.last_price.map(|p| p.to_string()),
                            }))
                        })
                        .collect()
                };
                v["books"] = to_json(&books);
                if socket
                    .send(Message::Text(v.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_actions::{Action, Transfer, CHAIN_ID_DEVNET};
    use keel_crypto::Keypair;
    use keel_types::Asset;
    use keel_vm::Genesis;

    struct Fake {
        state: Arc<Mutex<State>>,
        tx: broadcast::Sender<BlockUpdate>,
        submitted: Mutex<Vec<SignedAction>>,
    }

    impl NodeApi for Fake {
        fn state(&self) -> Arc<Mutex<State>> {
            self.state.clone()
        }
        fn submit(&self, action: SignedAction) -> Result<[u8; 32], VmError> {
            keel_vm::check_admission(&self.state.lock().unwrap(), &action)?;
            let id = action.id();
            self.submitted.lock().unwrap().push(action);
            Ok(id)
        }
        fn mempool_len(&self) -> usize {
            self.submitted.lock().unwrap().len()
        }
        fn receipt(&self, _: &[u8; 32]) -> Option<Receipt> {
            None
        }
        fn receipts_at(&self, _: u64) -> Option<Vec<Receipt>> {
            None
        }
        fn subscribe(&self) -> broadcast::Receiver<BlockUpdate> {
            self.tx.subscribe()
        }
        fn validators(&self) -> Vec<String> {
            vec!["v0".into()]
        }
    }

    async fn get_json(app: &Router, path: &str) -> (StatusCode, Value) {
        use axum::body::Body;
        use tower::ServiceExt as _;
        let resp = app
            .clone()
            .oneshot(axum::http::Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn status_account_and_submit() {
        let alice = Keypair::from_seed(1);
        let g = Genesis::devnet(CHAIN_ID_DEVNET, &[alice.address()], vec![]);
        let state = Arc::new(Mutex::new(g.build()));
        let (tx, _rx) = broadcast::channel(8);
        let node: Node = Arc::new(Fake {
            state,
            tx,
            submitted: Mutex::new(vec![]),
        });
        let app = router(node.clone());

        let (st, v) = get_json(&app, "/v1/status").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["chain_id"], CHAIN_ID_DEVNET);

        let (st, v) = get_json(&app, &format!("/v1/accounts/{}", alice.address().to_hex())).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["nonce"], 0);
        assert!(v["balances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["asset"] == "KEEL"));

        let (st, v) = get_json(&app, "/v1/markets/BTC-KUSD/book?depth=5").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["symbol"], "BTC-KUSD");
        let (st, _) = get_json(&app, "/v1/markets/NOPE/book").await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // Submit a transfer.
        use axum::body::Body;
        use tower::ServiceExt as _;
        let sa = SignedAction::sign(
            &alice,
            0,
            CHAIN_ID_DEVNET,
            Action::Transfer(Transfer {
                to: Address::tagged(2),
                asset: Asset::new("KEEL"),
                amount: 1,
                memo: None,
            }),
        );
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::post("/v1/actions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&sa).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(node.mempool_len(), 1);
        // Wrong nonce is refused with a code.
        let bad = SignedAction::sign(&alice, 5, CHAIN_ID_DEVNET, Action::BuyBudget { actions: 1 });
        let resp = app
            .oneshot(
                axum::http::Request::post("/v1/actions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&bad).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
