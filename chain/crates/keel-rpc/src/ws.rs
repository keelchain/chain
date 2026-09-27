//! `/v1/ws`: filtered, sequenced subscriptions over one socket.
//!
//! Client → server (JSON text frames):
//!
//! ```json
//! {"op":"subscribe","channels":["blocks","account:<hex>","pair:BTC-KUSD","deposits:BTC:<hex>","book:BTC-KUSD?depth=20"],"from_height":120}
//! {"op":"unsubscribe","channels":["pair:BTC-KUSD"]}
//! {"op":"ping"}
//! ```
//!
//! Server → client, every message with a per-connection `seq` (monotonic,
//! starting at 1) and the block `height` it belongs to:
//!
//! - `subscribed` / `unsubscribed`: the channel set now in force.
//! - `block` (channel `blocks`): the whole block as before: `timestamp`,
//!   `state_hash`, `receipts`, `events`, `books`.
//! - `event` (channels `account:`, `pair:`, `deposits:`): one receipt or one
//!   end-of-block event that touches the subscription, in `data`.
//! - `book_top` (channel `pair:`): best bid, best ask, last price after the block.
//! - `book_snapshot` then `book_delta` (channel `book:`): the top `depth`
//!   levels, then per block only the levels that changed (`size` 0 = gone).
//! - `gap`: this connection fell behind the block stream and messages were
//!   dropped; `from_height`..=`to_height` were not delivered and `resync`
//!   names the REST routes to read them from. Book channels get a fresh
//!   snapshot right after.
//! - `heartbeat` every 15 s; `pong` for a ping; `error` for a bad op.
//!
//! A connection that never subscribes receives `blocks`, so clients written
//! against the earlier firehose keep working.

use crate::{receipt_json, to_json, Node};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State as AxumState,
    },
    response::IntoResponse,
};
use futures::{SinkExt as _, StreamExt as _};
use keel_types::{Address, Side};
use keel_vm::{receipt::Event, Receipt};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::broadcast;

const HEARTBEAT_SECS: u64 = 15;
const MAX_CHANNELS: usize = 64;
const MAX_REPLAY: u64 = 10_000;

/// Book levels as sent: (side, price) -> size, side 0 = bids, 1 = asks.
type Levels = BTreeMap<(u8, String), String>;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Channel {
    Blocks,
    Account(Address),
    Pair(String),
    Deposits { chain: String, owner: Address },
    Book { pair: String, depth: usize },
}

impl Channel {
    pub(crate) fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s == "blocks" {
            return Ok(Channel::Blocks);
        }
        if let Some(rest) = s.strip_prefix("account:") {
            return Address::from_hex(rest)
                .map(Channel::Account)
                .ok_or_else(|| format!("account channel needs a 64-hex address: {s}"));
        }
        if let Some(rest) = s.strip_prefix("pair:") {
            if rest.is_empty() {
                return Err("pair channel needs a pair".into());
            }
            return Ok(Channel::Pair(rest.to_string()));
        }
        if let Some(rest) = s.strip_prefix("deposits:") {
            let (chain, owner) = rest
                .split_once(':')
                .ok_or_else(|| format!("deposits channel is deposits:<chain>:<address>: {s}"))?;
            let owner = Address::from_hex(owner)
                .ok_or_else(|| format!("deposits channel needs a 64-hex address: {s}"))?;
            return Ok(Channel::Deposits {
                chain: chain.to_ascii_uppercase(),
                owner,
            });
        }
        if let Some(rest) = s.strip_prefix("book:") {
            let (pair, depth) = match rest.split_once("?depth=") {
                Some((p, d)) => (
                    p,
                    d.parse::<usize>()
                        .map_err(|_| format!("book depth must be a number: {s}"))?,
                ),
                None => (rest, 20),
            };
            if pair.is_empty() {
                return Err("book channel needs a pair".into());
            }
            return Ok(Channel::Book {
                pair: pair.to_string(),
                depth: depth.clamp(1, 200),
            });
        }
        Err(format!("unknown channel {s}"))
    }

    fn name(&self) -> String {
        match self {
            Channel::Blocks => "blocks".into(),
            Channel::Account(a) => format!("account:{}", a.to_hex()),
            Channel::Pair(p) => format!("pair:{p}"),
            Channel::Deposits { chain, owner } => format!("deposits:{chain}:{}", owner.to_hex()),
            Channel::Book { pair, depth } => format!("book:{pair}?depth={depth}"),
        }
    }
}

/// Addresses an event concerns (for `account:` channels).
pub(crate) fn event_addresses(e: &Event) -> Vec<Address> {
    match e {
        Event::Transferred { from, to, .. } => vec![*from, *to],
        Event::OrderAccepted { owner, .. }
        | Event::BudgetPurchased { owner, .. }
        | Event::BudgetLocked { owner, .. }
        | Event::BudgetUnlockQueued { owner, .. }
        | Event::BudgetUnlocked { owner, .. }
        | Event::OfferCreated { owner, .. }
        | Event::DepositAddressAssigned { owner, .. }
        | Event::DepositCredited { owner, .. }
        | Event::WithdrawalQueued { owner, .. }
        | Event::StableMinted { owner, .. }
        | Event::StableBurned { owner, .. }
        | Event::Bonded { owner, .. }
        | Event::Unbonded { owner, .. }
        | Event::RewardsClaimed { owner, .. }
        | Event::Slashed { owner, .. } => vec![*owner],
        Event::CustodyVaultRegistered { custodian, .. }
        | Event::CustodyReserveBreached { custodian, .. } => vec![*custodian],
        Event::CustodyAddressAssigned {
            custodian, owner, ..
        }
        | Event::CustodyDepositCredited {
            custodian, owner, ..
        }
        | Event::CustodyWithdrawalQueued {
            custodian, owner, ..
        } => vec![*owner, *custodian],
        Event::Delegated {
            owner, validator, ..
        } => vec![*owner, *validator],
        Event::TradeStarted { buyer, seller, .. } => vec![*buyer, *seller],
        Event::TradeCancelled { by, .. } | Event::DisputeOpened { by, .. } => vec![*by],
        Event::ProposalCreated { proposer, .. } => vec![*proposer],
        Event::Voted { voter, .. } => vec![*voter],
        _ => Vec::new(),
    }
}

fn event_pair(e: &Event) -> Option<&str> {
    match e {
        Event::OrderAccepted { pair, .. }
        | Event::OrderFilled { pair, .. }
        | Event::OrderRejected { pair, .. } => Some(pair),
        _ => None,
    }
}

/// `(owner, chain)` of a deposit-side event, for `deposits:` channels.
fn event_deposit(e: &Event) -> Option<(Address, String)> {
    match e {
        Event::DepositAddressAssigned { owner, chain, .. }
        | Event::CustodyAddressAssigned { owner, chain, .. } => Some((*owner, chain.clone())),
        Event::DepositCredited { owner, asset, .. }
        | Event::WithdrawalQueued { owner, asset, .. }
        | Event::CustodyDepositCredited { owner, asset, .. }
        | Event::CustodyWithdrawalQueued { owner, asset, .. } => {
            asset.chain().map(|c| (*owner, c.to_string()))
        }
        _ => None,
    }
}

fn receipt_touches(r: &Receipt, addr: &Address) -> bool {
    r.signer == *addr || r.events.iter().any(|e| event_addresses(e).contains(addr))
}

struct Conn {
    seq: u64,
    channels: BTreeSet<Channel>,
    /// Last block height this connection was told about.
    last_height: u64,
    /// Last book levels sent per `book:` channel: (side, price) -> size.
    books: BTreeMap<String, Levels>,
}

impl Conn {
    fn next(&mut self, mut v: Value) -> Value {
        self.seq += 1;
        v["seq"] = json!(self.seq);
        v
    }
}

pub(crate) async fn ws(
    ws: WebSocketUpgrade,
    AxumState(node): AxumState<Node>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| run(socket, node))
}

async fn run(socket: WebSocket, node: Node) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = node.subscribe();
    let mut conn = Conn {
        seq: 0,
        channels: BTreeSet::from([Channel::Blocks]),
        last_height: node.state().lock().map(|s| s.height).unwrap_or(0),
        books: BTreeMap::new(),
    };
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_SECS));
    heartbeat.tick().await;
    let mut explicit = false;
    loop {
        let out: Vec<Value> = tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let (msgs, subscribed) = handle_op(&node, &mut conn, &text);
                    if subscribed { explicit = true; }
                    msgs
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_))) => Vec::new(),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
            },
            update = rx.recv() => match update {
                Ok(u) => {
                    let msgs = block_messages(&node, &mut conn, &u);
                    conn.last_height = u.height;
                    msgs
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => gap_messages(&node, &mut conn, missed),
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = heartbeat.tick() => vec![conn.next(json!({ "type": "heartbeat", "height": conn.last_height }))],
        };
        let _ = explicit;
        for m in out {
            if sink
                .send(Message::Text(m.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

/// Handles one client frame; returns the replies and whether it was a
/// subscription op.
fn handle_op(node: &Node, conn: &mut Conn, text: &str) -> (Vec<Value>, bool) {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (vec![conn.next(json!({ "type": "error", "message": format!("bad json: {e}"), "height": conn.last_height }))], false),
    };
    match v.get("op").and_then(Value::as_str) {
        Some("ping") => (vec![conn.next(json!({ "type": "pong", "height": conn.last_height }))], false),
        Some("subscribe") => {
            let mut wanted = BTreeSet::new();
            for c in v.get("channels").and_then(Value::as_array).into_iter().flatten() {
                match c.as_str().map(Channel::parse) {
                    Some(Ok(ch)) => {
                        wanted.insert(ch);
                    }
                    Some(Err(e)) => return (vec![conn.next(json!({ "type": "error", "message": e, "height": conn.last_height }))], false),
                    None => return (vec![conn.next(json!({ "type": "error", "message": "channels must be strings", "height": conn.last_height }))], false),
                }
            }
            if wanted.len() > MAX_CHANNELS {
                return (vec![conn.next(json!({ "type": "error", "message": format!("at most {MAX_CHANNELS} channels"), "height": conn.last_height }))], false);
            }
            conn.channels = wanted;
            conn.books.retain(|k, _| conn.channels.iter().any(|c| c.name() == *k));
            let mut out = vec![conn.next(json!({
                "type": "subscribed",
                "channels": conn.channels.iter().map(Channel::name).collect::<Vec<_>>(),
                "height": conn.last_height,
            }))];
            if let Some(from) = v.get("from_height").and_then(Value::as_u64) {
                out.extend(replay(node, conn, from));
            }
            out.extend(book_snapshots(node, conn));
            (out, true)
        }
        Some("unsubscribe") => {
            for c in v.get("channels").and_then(Value::as_array).into_iter().flatten() {
                if let Some(Ok(ch)) = c.as_str().map(Channel::parse) {
                    conn.channels.remove(&ch);
                    conn.books.remove(&ch.name());
                }
            }
            (vec![conn.next(json!({
                "type": "unsubscribed",
                "channels": conn.channels.iter().map(Channel::name).collect::<Vec<_>>(),
                "height": conn.last_height,
            }))], false)
        }
        other => (vec![conn.next(json!({ "type": "error", "message": format!("unknown op {other:?}"), "height": conn.last_height }))], false),
    }
}

/// Messages for one finalized block, per subscribed channel.
fn block_messages(node: &Node, conn: &mut Conn, u: &crate::BlockUpdate) -> Vec<Value> {
    let mut out = Vec::new();
    let height = u.height;
    let receipts_json: Vec<Value> = u.receipts.iter().map(receipt_json).collect();
    for ch in conn.channels.clone() {
        match &ch {
            Channel::Blocks => {
                let mut v = to_json(u);
                v["receipts"] = Value::Array(receipts_json.clone());
                v["books"] = to_json(&top_of_books(node));
                v["type"] = json!("block");
                v["channel"] = json!("blocks");
                out.push(conn.next(v));
            }
            Channel::Account(addr) => {
                for (r, rj) in u.receipts.iter().zip(&receipts_json) {
                    if receipt_touches(r, addr) {
                        out.push(conn.next(json!({ "type": "event", "channel": ch.name(), "height": height, "data": { "kind": "receipt", "receipt": rj } })));
                    }
                }
                for e in &u.events {
                    if event_addresses(e).contains(addr) {
                        out.push(conn.next(json!({ "type": "event", "channel": ch.name(), "height": height, "data": { "kind": "event", "event": to_json(e) } })));
                    }
                }
            }
            Channel::Pair(pair) => {
                for r in &u.receipts {
                    for e in &r.events {
                        if event_pair(e) == Some(pair.as_str()) {
                            out.push(conn.next(json!({ "type": "event", "channel": ch.name(), "height": height, "data": { "kind": "event", "tx_id": hex::encode(r.tx_id), "event": to_json(e) } })));
                        }
                    }
                }
                if let Some(top) = top_of_book(node, pair) {
                    let mut v = top;
                    v["type"] = json!("book_top");
                    v["channel"] = json!(ch.name());
                    v["height"] = json!(height);
                    out.push(conn.next(v));
                }
            }
            Channel::Deposits { chain, owner } => {
                let all = u
                    .receipts
                    .iter()
                    .flat_map(|r| r.events.iter())
                    .chain(u.events.iter());
                for e in all {
                    if let Some((o, c)) = event_deposit(e) {
                        if o == *owner && c.eq_ignore_ascii_case(chain) {
                            out.push(conn.next(json!({ "type": "event", "channel": ch.name(), "height": height, "data": { "kind": "event", "event": to_json(e) } })));
                        }
                    }
                }
            }
            Channel::Book { pair, depth } => {
                if let Some(m) = book_delta(node, conn, &ch, pair, *depth, height) {
                    out.push(m);
                }
            }
        }
    }
    out
}

fn gap_messages(node: &Node, conn: &mut Conn, missed: u64) -> Vec<Value> {
    let tip = node
        .state()
        .lock()
        .map(|s| s.height)
        .unwrap_or(conn.last_height);
    let from = conn.last_height + 1;
    let to = tip.max(from);
    let mut out = vec![conn.next(json!({
        "type": "gap",
        "missed": missed,
        "from_height": from,
        "to_height": to,
        "height": tip,
        "resync": {
            "receipts": "/v1/blocks/{height}/receipts",
            "book": "/v1/markets/{pair}/book",
            "account": "/v1/accounts/{address}",
        },
    }))];
    conn.last_height = tip;
    conn.books.clear();
    out.extend(book_snapshots(node, conn));
    out
}

/// Deliver history for `account:`, `pair:` and `deposits:` channels from
/// `from` up to the tip, out of the archive (receipts) and the receipt
/// window (end-of-block events).
fn replay(node: &Node, conn: &mut Conn, from: u64) -> Vec<Value> {
    let tip = node.state().lock().map(|s| s.height).unwrap_or(0);
    let start = from.max(tip.saturating_sub(MAX_REPLAY)).max(1);
    let mut out = Vec::new();
    for h in start..=tip {
        let receipts = node
            .receipts_at(h)
            .or_else(|| node.receipts_archived(h))
            .unwrap_or_default();
        let events = node.events_at(h).unwrap_or_default();
        let meta = node.block_meta(h);
        let update = crate::BlockUpdate {
            height: h,
            timestamp: meta.as_ref().map(|m| m.timestamp).unwrap_or(0),
            state_hash: meta.and_then(|m| m.state_hash).unwrap_or_default(),
            receipts,
            events,
        };
        // Books and block messages are live-only; keep the event channels.
        let live: BTreeSet<Channel> = conn
            .channels
            .iter()
            .filter(|c| !matches!(c, Channel::Blocks | Channel::Book { .. }))
            .cloned()
            .collect();
        let saved = std::mem::replace(&mut conn.channels, live);
        out.extend(block_messages(node, conn, &update));
        conn.channels = saved;
    }
    out
}

fn top_of_books(node: &Node) -> BTreeMap<String, Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    s.markets
        .pairs
        .iter()
        .map(|(k, m)| {
            (
                k.clone(),
                json!({
                    "best_bid": m.book.best_price(Side::Buy).map(|p| p.to_string()),
                    "best_ask": m.book.best_price(Side::Sell).map(|p| p.to_string()),
                    "last_price": m.last_price.map(|p| p.to_string()),
                }),
            )
        })
        .collect()
}

fn top_of_book(node: &Node, pair: &str) -> Option<Value> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let m = s.markets.pairs.get(pair)?;
    Some(json!({
        "pair": pair,
        "best_bid": m.book.best_price(Side::Buy).map(|p| p.to_string()),
        "best_ask": m.book.best_price(Side::Sell).map(|p| p.to_string()),
        "last_price": m.last_price.map(|p| p.to_string()),
    }))
}

/// Current top-`depth` levels as (side, price) -> size, plus the height.
fn book_levels(node: &Node, pair: &str, depth: usize) -> Option<(u64, Levels)> {
    let state = node.state();
    let s = state.lock().expect("state lock");
    let m = s.markets.pairs.get(pair)?;
    let mut levels = BTreeMap::new();
    for (side, tag) in [(Side::Buy, 0u8), (Side::Sell, 1u8)] {
        for l in m.book.levels(side, depth) {
            levels.insert((tag, l.price.to_string()), l.size.to_string());
        }
    }
    Some((s.height, levels))
}

fn snapshot_json(levels: &Levels) -> (Vec<Value>, Vec<Value>) {
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    for ((side, price), size) in levels {
        let row = json!([price, size]);
        if *side == 0 {
            bids.push(row);
        } else {
            asks.push(row);
        }
    }
    // Bids best-first (highest price), asks best-first (lowest price).
    bids.reverse();
    (bids, asks)
}

fn book_snapshots(node: &Node, conn: &mut Conn) -> Vec<Value> {
    let mut out = Vec::new();
    for ch in conn.channels.clone() {
        if let Channel::Book { pair, depth } = &ch {
            if let Some((height, levels)) = book_levels(node, pair, *depth) {
                let (bids, asks) = snapshot_json(&levels);
                conn.books.insert(ch.name(), levels);
                out.push(conn.next(json!({ "type": "book_snapshot", "channel": ch.name(), "pair": pair, "height": height, "bids": bids, "asks": asks })));
            }
        }
    }
    out
}

fn book_delta(
    node: &Node,
    conn: &mut Conn,
    ch: &Channel,
    pair: &str,
    depth: usize,
    height: u64,
) -> Option<Value> {
    let (_, now) = book_levels(node, pair, depth)?;
    let name = ch.name();
    let Some(prev) = conn.books.get(&name) else {
        let (bids, asks) = snapshot_json(&now);
        conn.books.insert(name.clone(), now);
        return Some(conn.next(json!({ "type": "book_snapshot", "channel": name, "pair": pair, "height": height, "bids": bids, "asks": asks })));
    };
    let mut changes = Vec::new();
    for (k, size) in &now {
        if prev.get(k) != Some(size) {
            changes.push(json!([if k.0 == 0 { "buy" } else { "sell" }, k.1, size]));
        }
    }
    for k in prev.keys() {
        if !now.contains_key(k) {
            changes.push(json!([if k.0 == 0 { "buy" } else { "sell" }, k.1, "0"]));
        }
    }
    conn.books.insert(name.clone(), now);
    if changes.is_empty() {
        return None;
    }
    Some(conn.next(json!({ "type": "book_delta", "channel": name, "pair": pair, "height": height, "changes": changes })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_types::Asset;

    #[test]
    fn channels_parse_and_print() {
        let a = Address::tagged(7);
        let hex = a.to_hex();
        assert_eq!(Channel::parse("blocks").unwrap(), Channel::Blocks);
        assert_eq!(
            Channel::parse(&format!("account:{hex}")).unwrap(),
            Channel::Account(a)
        );
        assert_eq!(
            Channel::parse("pair:BTC-KUSD").unwrap(),
            Channel::Pair("BTC-KUSD".into())
        );
        assert_eq!(
            Channel::parse(&format!("deposits:btc:{hex}")).unwrap(),
            Channel::Deposits {
                chain: "BTC".into(),
                owner: a
            }
        );
        assert_eq!(
            Channel::parse("book:BTC-KUSD?depth=5").unwrap(),
            Channel::Book {
                pair: "BTC-KUSD".into(),
                depth: 5
            }
        );
        assert_eq!(
            Channel::parse("book:BTC-KUSD").unwrap().name(),
            "book:BTC-KUSD?depth=20"
        );
        assert!(Channel::parse("account:zz").is_err());
        assert!(Channel::parse("deposits:BTC").is_err());
        assert!(Channel::parse("nope").is_err());
    }

    #[test]
    fn events_map_to_addresses_pairs_and_chains() {
        let a = Address::tagged(1);
        let b = Address::tagged(2);
        let t = Event::Transferred {
            from: a,
            to: b,
            asset: Asset::new("KEEL"),
            amount: 1,
        };
        assert_eq!(event_addresses(&t), vec![a, b]);
        let o = Event::OrderAccepted {
            order_id: 1,
            owner: a,
            pair: "BTC-KUSD".into(),
            resting: 0,
        };
        assert_eq!(event_pair(&o), Some("BTC-KUSD"));
        let d = Event::DepositCredited {
            owner: a,
            asset: Asset::vault("BTC", "BTC"),
            amount: 5,
        };
        assert_eq!(event_deposit(&d), Some((a, "BTC".into())));
        let w = Event::WithdrawalQueued {
            outbound_id: 1,
            owner: b,
            asset: Asset::vault("TRON", "USDT"),
            amount: 5,
            to: "T..".into(),
        };
        assert_eq!(event_deposit(&w), Some((b, "TRON".into())));
        assert_eq!(event_deposit(&t), None);
    }

    #[test]
    fn snapshot_orders_best_first() {
        let mut levels = BTreeMap::new();
        levels.insert((0u8, "100".to_string()), "1".to_string());
        levels.insert((0u8, "200".to_string()), "2".to_string());
        levels.insert((1u8, "300".to_string()), "3".to_string());
        levels.insert((1u8, "400".to_string()), "4".to_string());
        let (bids, asks) = snapshot_json(&levels);
        assert_eq!(bids[0], json!(["200", "2"]));
        assert_eq!(asks[0], json!(["300", "3"]));
    }
}
