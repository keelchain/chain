//! Health, stats, blocks, txs, accounts, search and the explorer WebSocket.

use super::{
    decimals_map, jcol, jrow, page, parse_hex64, with_events, ApiError, ApiResult, App, Cursor,
    Paging, BLOCK_COLS, TX_COLS,
};
use crate::node::status as node_status;
use crate::types::{field_str, is_hex64};
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::atomic::Ordering;

pub async fn health(State(app): State<App>) -> ApiResult {
    let s = &app.status;
    let node_height = match node_status(&*app.node).await {
        Ok(ns) => {
            s.node_height.store(ns.height, Ordering::Relaxed);
            // Live check: the node's current hash vs what we stored at that height.
            if let Ok(Some(Some(stored))) = app.db.block_state_hash(ns.height as i64).await {
                if !ns.state_hash.is_empty() && stored != ns.state_hash {
                    s.state_hash_ok.store(false, Ordering::Relaxed);
                    *s.mismatch.lock().expect("lock") = Some(crate::HashMismatch {
                        height: ns.height,
                        node: ns.state_hash.clone(),
                        stored,
                    });
                }
            }
            Some(ns.height)
        }
        Err(_) => None,
    };
    let indexed = s.indexed_height.load(Ordering::Relaxed);
    let mismatch = s.mismatch.lock().expect("lock").clone();
    Ok(Json(json!({
        "network": app.cfg.network,
        "chain_id": s.chain_id.load(Ordering::Relaxed),
        "indexed_height": indexed,
        "node_height": node_height,
        "node_reachable": node_height.is_some(),
        "lag": node_height.map(|h| h.saturating_sub(indexed)),
        "state_hash_ok": s.state_hash_ok.load(Ordering::Relaxed),
        "state_hash_mismatch": mismatch,
        "started_at": s.started_at.load(Ordering::Relaxed),
        "first_indexed_height": s.first_indexed_height.load(Ordering::Relaxed),
        "missing_blocks": s.missing_blocks.load(Ordering::Relaxed),
        "backfilling": s.backfilling.load(Ordering::Relaxed),
        "external_network": app.cfg.external_network,
        "last_error": s.last_error.lock().expect("lock").clone(),
    })))
}

pub async fn stats(State(app): State<App>) -> ApiResult {
    let db = &app.db.pool;
    let now = crate::now_ms();
    let height = app.status.indexed_height.load(Ordering::Relaxed);
    let bt = sqlx::query("SELECT (MAX(timestamp) - MIN(timestamp)) / NULLIF(COUNT(*) - 1, 0) AS avg FROM (SELECT timestamp FROM blocks ORDER BY height DESC LIMIT 1000) x")
        .fetch_one(db)
        .await?
        .try_get::<Option<i64>, _>("avg")?;
    let tx_1h: i64 = sqlx::query("SELECT COUNT(*) AS n FROM txs WHERE timestamp >= $1")
        .bind(now - 3_600_000)
        .fetch_one(db)
        .await?
        .get("n");
    let tx_24h: i64 = sqlx::query("SELECT COUNT(*) AS n FROM txs WHERE timestamp >= $1")
        .bind(now - 86_400_000)
        .fetch_one(db)
        .await?
        .get("n");
    let accounts: i64 = sqlx::query("SELECT COUNT(*) AS n FROM account_stats WHERE address <> $1")
        .bind(crate::materialize::SYSTEM_ADDR)
        .fetch_one(db)
        .await?
        .get("n");
    let validators: i64 = sqlx::query("SELECT COUNT(*) AS n FROM validators")
        .fetch_one(db)
        .await?
        .get("n");
    let assets: Vec<Value> = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT asset, decimals, kind, chain, supply::text AS supply, holders FROM assets ORDER BY asset) x").fetch_all(db).await?.iter().map(jrow).collect();
    let fees: Vec<Value> = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT asset, SUM(amount)::text AS amount FROM transfers WHERE kind IN ('fill_fee', 'trade_fee') AND timestamp >= $1 GROUP BY asset ORDER BY asset) x")
        .bind(now - 86_400_000)
        .fetch_all(db)
        .await?
        .iter()
        .map(jrow)
        .collect();
    // TVL: vault reserves (system vault_asset accounts) valued at the last
    // price of the asset's KUSD market, plus the stablecoin itself.
    let tvl = sqlx::query(
        "SELECT COALESCE(SUM(usd), 0)::text AS tvl FROM (
           SELECT b.asset, CASE WHEN a.kind = 'stable' THEN SUM(b.balance)
                                ELSE SUM(b.balance) * COALESCE((SELECT m.last_price FROM markets m WHERE m.base = b.asset AND m.quote = 'KUSD' LIMIT 1), 0) / power(10, a.decimals) END AS usd
           FROM balances b JOIN assets a ON a.asset = b.asset
           WHERE b.address = $1 AND b.account_type IN ('vault_asset', 'stable_reserve')
           GROUP BY b.asset, a.kind, a.decimals) x",
    )
    .bind(crate::materialize::SYSTEM_ADDR)
    .fetch_one(db)
    .await?
    .get::<String, _>("tvl");
    Ok(Json(json!({
        "height": height,
        "block_time_ms_avg": bt,
        "tps_1h": (tx_1h as f64) / 3600.0,
        "txs_1h": tx_1h,
        "actions_24h": tx_24h,
        "accounts": accounts,
        "validators": validators,
        "tvl_usd": micro_to_usd(&tvl),
        "assets": assets,
        "fees_24h": fees,
    })))
}

/// `"1234567890"` micro-USD → `"1234.567890"`.
fn micro_to_usd(micro: &str) -> String {
    let s = micro
        .split('.')
        .next()
        .unwrap_or("0")
        .trim_start_matches('-');
    let neg = micro.starts_with('-');
    let padded = format!("{:0>7}", s);
    let (int, frac) = padded.split_at(padded.len() - 6);
    format!("{}{}.{}", if neg { "-" } else { "" }, int, frac)
}

pub async fn blocks(State(app): State<App>, Query(p): Query<Paging>) -> ApiResult {
    let limit = p.limit();
    let before = Cursor::part(&p.cursor(1)?, 0);
    let rows: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {BLOCK_COLS} FROM blocks WHERE height < $1 ORDER BY height DESC LIMIT $2) x"))
        .bind(before)
        .bind(limit + 1)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    let (rows, next) = page(rows, limit, |b| Cursor(vec![jcol(b, "height")]));
    Ok(Json(json!({ "blocks": rows, "next_cursor": next })))
}

pub async fn block(State(app): State<App>, Path(height): Path<String>) -> ApiResult {
    let h: i64 = height
        .parse()
        .map_err(|_| ApiError::bad_request("height must be an integer"))?;
    let b = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {BLOCK_COLS} FROM blocks WHERE height = $1) x"
    ))
    .bind(h)
    .fetch_optional(&app.db.pool)
    .await?;
    let mut b = b
        .map(|r| jrow(&r))
        .ok_or_else(|| ApiError::not_found("block"))?;
    let txs: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {TX_COLS} FROM txs WHERE height = $1 ORDER BY index) x")).bind(h).fetch_all(&app.db.pool).await?.iter().map(jrow).collect();
    let txs = with_events(&app.db, txs).await?;
    let block_events: Vec<Value> = sqlx::query(
        "SELECT CASE WHEN jsonb_typeof(data) = 'object' THEN jsonb_build_object('type', type) || data ELSE jsonb_build_object('type', type, 'data', data) END AS j FROM events WHERE height = $1 AND tx_index = -1 ORDER BY event_index",
    )
    .bind(h)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    b["receipts"] = Value::Array(txs);
    b["events"] = Value::Array(block_events);
    Ok(Json(b))
}

#[derive(Deserialize)]
pub struct TxQuery {
    limit: Option<i64>,
    cursor: Option<String>,
    signer: Option<String>,
    module: Option<String>,
    kind: Option<String>,
    ok: Option<bool>,
}

pub async fn txs(State(app): State<App>, Query(q): Query<TxQuery>) -> ApiResult {
    let p = Paging {
        limit: q.limit,
        cursor: q.cursor.clone(),
    };
    let signer = match &q.signer {
        Some(s) => Some(parse_hex64(s, "signer")?),
        None => None,
    };
    list_txs(
        &app,
        &p,
        signer,
        None,
        q.module.clone(),
        q.kind.clone(),
        q.ok,
    )
    .await
}

async fn list_txs(
    app: &App,
    p: &Paging,
    signer: Option<String>,
    counterparty: Option<String>,
    module: Option<String>,
    kind: Option<String>,
    ok: Option<bool>,
) -> ApiResult {
    let limit = p.limit();
    let c = p.cursor(2)?;
    let (ch, ci) = (Cursor::part(&c, 0), Cursor::part(&c, 1));
    let sql = format!(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT {TX_COLS} FROM txs t
           WHERE (height, index) < ($1, $2)
             AND ($3::text IS NULL OR signer = $3 OR ($4::text IS NOT NULL AND EXISTS (SELECT 1 FROM events e WHERE e.height = t.height AND e.tx_index = t.index AND e.addresses @> ARRAY[$4::text])))
             AND ($5::text IS NULL OR module = $5) AND ($6::text IS NULL OR kind = $6) AND ($7::bool IS NULL OR ok = $7)
           ORDER BY height DESC, index DESC LIMIT $8) x"
    );
    let rows: Vec<Value> = sqlx::query(&sql)
        .bind(ch)
        .bind(ci as i32)
        .bind(&signer)
        .bind(&counterparty)
        .bind(&module)
        .bind(&kind)
        .bind(ok)
        .bind(limit + 1)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    let (rows, next) = page(rows, limit, |t| {
        Cursor(vec![jcol(t, "height"), jcol(t, "index")])
    });
    let rows = with_events(&app.db, rows).await?;
    Ok(Json(json!({ "txs": rows, "next_cursor": next })))
}

pub async fn tx(State(app): State<App>, Path(tx_id): Path<String>) -> ApiResult {
    let id = parse_hex64(&tx_id, "tx id")?;
    // Duplicate inclusion: prefer the receipt that consumed the nonce (the
    // one that is not BAD_NONCE noise), then the earliest.
    let row = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {TX_COLS}, action FROM txs WHERE tx_id = $1 ORDER BY (error_code = 'BAD_NONCE') NULLS FIRST, height, index LIMIT 1) x"
    ))
    .bind(&id)
    .fetch_optional(&app.db.pool)
    .await?;
    let mut t = row
        .map(|r| jrow(&r))
        .ok_or_else(|| ApiError::not_found("tx"))?;
    let (h, i) = (jcol(&t, "height"), jcol(&t, "index"));
    let mut evs = super::events_for(&app.db, &[(h, i)]).await?;
    t["events"] = Value::Array(evs.remove(&(h, i)).unwrap_or_default());
    let b = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {BLOCK_COLS} FROM blocks WHERE height = $1) x"
    ))
    .bind(h)
    .fetch_optional(&app.db.pool)
    .await?;
    t["block"] = b.map(|r| jrow(&r)).unwrap_or(Value::Null);
    let dups: i64 = sqlx::query("SELECT COUNT(*) AS n FROM txs WHERE tx_id = $1")
        .bind(&id)
        .fetch_one(&app.db.pool)
        .await?
        .get("n");
    if dups > 1 {
        t["inclusions"] = json!(dups);
    }
    Ok(Json(t))
}

pub async fn account(State(app): State<App>, Path(addr): Path<String>) -> ApiResult {
    let a = parse_hex64(&addr, "address")?;
    let node_acc = app
        .node
        .get(&format!("/v1/accounts/{a}"))
        .await
        .ok()
        .flatten();
    let stats = sqlx::query("SELECT first_seen_height, last_seen_height, tx_count FROM account_stats WHERE address = $1").bind(&a).fetch_optional(&app.db.pool).await?;
    if node_acc.is_none() && stats.is_none() {
        return Err(ApiError::not_found("account"));
    }
    let decimals = decimals_map(&app.db).await?;
    let mut balances: Vec<Value> = node_acc
        .as_ref()
        .and_then(|n| n.get("balances"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if node_acc.is_none() {
        // Node unreachable: last reconciled balances.
        balances = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT asset, account_type, balance::text AS balance FROM balances WHERE address = $1 ORDER BY asset, account_type) x")
            .bind(&a)
            .fetch_all(&app.db.pool)
            .await?
            .iter()
            .map(jrow)
            .collect();
    }
    for b in &mut balances {
        let asset = field_str(b, "asset").unwrap_or_default();
        b["decimals"] = json!(decimals.get(&asset).copied());
        if let Some(v) = b.get("balance").and_then(crate::types::amount_of) {
            b["balance"] = json!(v.to_string());
        }
    }
    let deposit_rows = sqlx::query("SELECT data FROM events WHERE type = 'DepositAddressAssigned' AND addresses @> ARRAY[$1::text] ORDER BY height").bind(&a).fetch_all(&app.db.pool).await?;
    let mut deposit_addresses = Vec::new();
    for r in deposit_rows {
        let d: Value = r.get("data");
        let chain = field_str(&d, "chain").unwrap_or_default();
        let index = d.get("index").and_then(Value::as_u64).unwrap_or(0);
        let address = app
            .node
            .get(&format!("/v1/vaults/{chain}/addresses/{index}"))
            .await
            .ok()
            .flatten()
            .and_then(|v| field_str(&v, "address"));
        deposit_addresses.push(json!({ "chain": chain, "index": index, "address": address }));
    }
    let validator = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT address, consensus_key, self_bond::text AS self_bond, delegated::text AS delegated, power::text AS power, jailed, in_consensus, joined_epoch FROM validators WHERE address = $1) x")
        .bind(&a)
        .fetch_optional(&app.db.pool)
        .await?
        .map(|r| jrow(&r));
    let offers: i64 = sqlx::query("SELECT COUNT(*) AS n FROM offers WHERE owner = $1")
        .bind(&a)
        .fetch_one(&app.db.pool)
        .await?
        .get("n");
    let trades: i64 =
        sqlx::query("SELECT COUNT(*) AS n FROM trades WHERE buyer = $1 OR seller = $1")
            .bind(&a)
            .fetch_one(&app.db.pool)
            .await?
            .get("n");
    let n = node_acc.unwrap_or(Value::Null);
    Ok(Json(json!({
        "address": a,
        "nonce": n.get("nonce").cloned().unwrap_or(json!(0)),
        "tier": n.get("tier").cloned().unwrap_or(json!(0)),
        "tier_expires_at": n.get("tier_expires_at").cloned(),
        "budget": n.get("budget").cloned(),
        "balances": balances,
        "first_seen_height": stats.as_ref().map(|r| r.get::<i64, _>("first_seen_height")),
        "last_seen_height": stats.as_ref().map(|r| r.get::<i64, _>("last_seen_height")),
        "tx_count": stats.as_ref().map(|r| r.get::<i64, _>("tx_count")).unwrap_or(0),
        "deposit_addresses": deposit_addresses,
        "validator": validator,
        "offers_count": offers,
        "trades_count": trades,
    })))
}

pub async fn account_txs(
    State(app): State<App>,
    Path(addr): Path<String>,
    Query(q): Query<TxQuery>,
) -> ApiResult {
    let a = parse_hex64(&addr, "address")?;
    let p = Paging {
        limit: q.limit,
        cursor: q.cursor.clone(),
    };
    list_txs(
        &app,
        &p,
        Some(a.clone()),
        Some(a),
        q.module.clone(),
        q.kind.clone(),
        q.ok,
    )
    .await
}

#[derive(Deserialize)]
pub struct TransferQuery {
    limit: Option<i64>,
    cursor: Option<String>,
    asset: Option<String>,
    kind: Option<String>,
}

pub async fn account_transfers(
    State(app): State<App>,
    Path(addr): Path<String>,
    Query(q): Query<TransferQuery>,
) -> ApiResult {
    let a = parse_hex64(&addr, "address")?;
    let p = Paging {
        limit: q.limit,
        cursor: q.cursor.clone(),
    };
    let limit = p.limit();
    let c = p.cursor(4)?;
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT t.height, t.tx_index, t.event_index, t.leg, t.tx_id, t.timestamp, t.asset, t.amount::text AS amount, t.from_addr AS \"from\", t.to_addr AS \"to\", t.kind, a.decimals,
                  COALESCE(d.tx_hash, o.tx_hash) AS external_tx_hash, COALESCE(d.chain, o.chain, a.chain) AS external_chain, o.to_addr AS external_to
           FROM transfers t LEFT JOIN assets a ON a.asset = t.asset
             LEFT JOIN LATERAL (SELECT tx_hash, chain FROM deposits d WHERE t.kind = 'deposit' AND d.tx_id = t.tx_id AND d.owner = $1 LIMIT 1) d ON true
             LEFT JOIN LATERAL (SELECT tx_hash, chain, to_addr FROM outbounds o WHERE t.kind IN ('withdrawal', 'lightning_payout') AND o.tx_id = t.tx_id AND o.owner = $1 LIMIT 1) o ON true
           WHERE (t.from_addr = $1 OR t.to_addr = $1) AND (t.height, t.tx_index, t.event_index, t.leg) < ($2, $3, $4, $5)
             AND ($6::text IS NULL OR t.asset = $6) AND ($7::text IS NULL OR t.kind = $7)
           ORDER BY t.height DESC, t.tx_index DESC, t.event_index DESC, t.leg DESC LIMIT $8) x",
    )
    .bind(&a)
    .bind(Cursor::part(&c, 0))
    .bind(Cursor::part(&c, 1) as i32)
    .bind(Cursor::part(&c, 2) as i32)
    .bind(Cursor::part(&c, 3) as i16)
    .bind(&q.asset)
    .bind(&q.kind)
    .bind(limit + 1)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let (rows, next) = page(rows, limit, |t| {
        Cursor(vec![
            jcol(t, "height"),
            jcol(t, "tx_index"),
            jcol(t, "event_index"),
            jcol(t, "leg"),
        ])
    });
    Ok(Json(json!({ "transfers": rows, "next_cursor": next })))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

pub async fn search(State(app): State<App>, Query(sq): Query<SearchQuery>) -> ApiResult {
    let q = sq.q.unwrap_or_default().trim().to_string();
    if q.is_empty() {
        return Err(ApiError::bad_request("q is required"));
    }
    let db = &app.db.pool;
    let mut suggestions: Vec<Value> = Vec::new();
    let hit = |kind: &str, r: Value| json!({ "kind": kind, "ref": r });
    // Prefixed ids: offer:12, trade:7, order:3, proposal:1, block:5
    if let Some((prefix, id)) = q.split_once(':') {
        let id: i64 = id
            .trim()
            .parse()
            .map_err(|_| ApiError::bad_request("id must be an integer"))?;
        let (table, kind) = match prefix.trim().to_ascii_lowercase().as_str() {
            "offer" => ("offers", "offer"),
            "trade" => ("trades", "trade"),
            "order" => ("orders", "order"),
            "proposal" => ("proposals", "proposal"),
            "block" => ("blocks", "block"),
            _ => {
                return Err(ApiError::bad_request(
                    "unknown prefix (offer, trade, order, proposal, block)",
                ))
            }
        };
        let col = if table == "blocks" { "height" } else { "id" };
        let found = sqlx::query(&format!("SELECT 1 AS x FROM {table} WHERE {col} = $1"))
            .bind(id)
            .fetch_optional(db)
            .await?
            .is_some();
        return if found {
            Ok(Json(
                json!({ "kind": kind, "ref": id.to_string(), "suggestions": [] }),
            ))
        } else {
            Err(ApiError::not_found(kind))
        };
    }
    // Height
    if q.bytes().all(|b| b.is_ascii_digit()) {
        let h: i64 = q
            .parse()
            .map_err(|_| ApiError::bad_request("height too large"))?;
        if sqlx::query("SELECT 1 AS x FROM blocks WHERE height = $1")
            .bind(h)
            .fetch_optional(db)
            .await?
            .is_some()
        {
            return Ok(Json(
                json!({ "kind": "block", "ref": h.to_string(), "suggestions": [] }),
            ));
        }
        if sqlx::query("SELECT 1 AS x FROM orders WHERE id = $1")
            .bind(h)
            .fetch_optional(db)
            .await?
            .is_some()
        {
            suggestions.push(hit("order", json!(h.to_string())));
        }
        if suggestions.is_empty() {
            return Err(ApiError::not_found("block"));
        }
        return Ok(Json(
            json!({ "kind": "order", "ref": h.to_string(), "suggestions": suggestions }),
        ));
    }
    let lower = q.trim_start_matches("0x").to_ascii_lowercase();
    // 64 hex: tx, then validator, then account
    if is_hex64(&lower) {
        if sqlx::query("SELECT 1 AS x FROM txs WHERE tx_id = $1")
            .bind(&lower)
            .fetch_optional(db)
            .await?
            .is_some()
        {
            return Ok(Json(
                json!({ "kind": "tx", "ref": lower, "suggestions": [] }),
            ));
        }
        let is_validator =
            sqlx::query("SELECT 1 AS x FROM validators WHERE address = $1 OR consensus_key = $1")
                .bind(&lower)
                .fetch_optional(db)
                .await?
                .is_some();
        let known = sqlx::query("SELECT 1 AS x FROM account_stats WHERE address = $1")
            .bind(&lower)
            .fetch_optional(db)
            .await?
            .is_some()
            || app
                .node
                .get(&format!("/v1/accounts/{lower}"))
                .await
                .ok()
                .flatten()
                .is_some_and(|v| {
                    v.get("balances")
                        .and_then(Value::as_array)
                        .is_some_and(|b| !b.is_empty())
                        || v.get("nonce").and_then(Value::as_u64).unwrap_or(0) > 0
                });
        if is_validator {
            if known {
                suggestions.push(hit("account", json!(lower)));
            }
            return Ok(Json(
                json!({ "kind": "validator", "ref": lower, "suggestions": suggestions }),
            ));
        }
        if known {
            return Ok(Json(
                json!({ "kind": "account", "ref": lower, "suggestions": [] }),
            ));
        }
        // External hashes: a deposit's source tx or a payout's broadcast tx
        // resolve to the chain tx that recorded them (credit / confirmation).
        if let Some(r) = sqlx::query("SELECT tx_id FROM deposits WHERE tx_hash = $1 AND tx_id IS NOT NULL ORDER BY updated_height DESC LIMIT 1").bind(&lower).fetch_optional(db).await? {
            return Ok(Json(json!({ "kind": "tx", "ref": r.get::<String, _>("tx_id"), "suggestions": [], "matched": "deposit_tx_hash" })));
        }
        if let Some(r) = sqlx::query("SELECT tx_id FROM outbounds WHERE tx_hash = $1 AND tx_id IS NOT NULL ORDER BY updated_height DESC LIMIT 1").bind(&lower).fetch_optional(db).await? {
            return Ok(Json(json!({ "kind": "tx", "ref": r.get::<String, _>("tx_id"), "suggestions": [], "matched": "outbound_tx_hash" })));
        }
        return Err(ApiError::not_found("tx or account"));
    }
    // Hex prefix (>= 6 chars): suggest txs / accounts
    if lower.len() >= 6 && lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        let like = format!("{lower}%");
        for r in sqlx::query("SELECT DISTINCT tx_id FROM txs WHERE tx_id LIKE $1 LIMIT 5")
            .bind(&like)
            .fetch_all(db)
            .await?
        {
            suggestions.push(hit("tx", json!(r.get::<String, _>("tx_id"))));
        }
        for r in sqlx::query("SELECT address FROM account_stats WHERE address LIKE $1 LIMIT 5")
            .bind(&like)
            .fetch_all(db)
            .await?
        {
            suggestions.push(hit("account", json!(r.get::<String, _>("address"))));
        }
        if let Some(first) = suggestions.first().cloned() {
            return Ok(Json(
                json!({ "kind": first["kind"], "ref": first["ref"], "suggestions": suggestions }),
            ));
        }
        return Err(ApiError::not_found("match"));
    }
    let upper = q.to_ascii_uppercase();
    // Pair
    if upper.contains('-') {
        if let Some(r) = sqlx::query("SELECT pair FROM markets WHERE UPPER(pair) = $1")
            .bind(&upper)
            .fetch_optional(db)
            .await?
        {
            return Ok(Json(
                json!({ "kind": "market", "ref": r.get::<String, _>("pair"), "suggestions": [] }),
            ));
        }
    }
    // Asset by id or symbol
    let assets: Vec<String> = sqlx::query("SELECT asset FROM assets WHERE UPPER(asset) = $1 OR UPPER(split_part(asset, '.', 2)) = $1 ORDER BY (UPPER(asset) = $1) DESC, asset")
        .bind(&upper)
        .fetch_all(db)
        .await?
        .iter()
        .map(|r| r.get::<String, _>("asset"))
        .collect();
    if let Some(first) = assets.first() {
        for a in assets.iter().skip(1) {
            suggestions.push(hit("asset", json!(a)));
        }
        for r in sqlx::query("SELECT pair FROM markets WHERE base = $1 OR quote = $1")
            .bind(first)
            .fetch_all(db)
            .await?
        {
            suggestions.push(hit("market", json!(r.get::<String, _>("pair"))));
        }
        return Ok(Json(
            json!({ "kind": "asset", "ref": first, "suggestions": suggestions }),
        ));
    }
    // Loose: pair / asset prefix suggestions
    let like = format!("{upper}%");
    for r in sqlx::query("SELECT pair FROM markets WHERE UPPER(pair) LIKE $1 LIMIT 5")
        .bind(&like)
        .fetch_all(db)
        .await?
    {
        suggestions.push(hit("market", json!(r.get::<String, _>("pair"))));
    }
    for r in sqlx::query("SELECT asset FROM assets WHERE UPPER(asset) LIKE $1 OR UPPER(split_part(asset, '.', 2)) LIKE $1 LIMIT 5").bind(&like).fetch_all(db).await? {
        suggestions.push(hit("asset", json!(r.get::<String, _>("asset"))));
    }
    if let Some(first) = suggestions.first().cloned() {
        return Ok(Json(
            json!({ "kind": first["kind"], "ref": first["ref"], "suggestions": suggestions }),
        ));
    }
    Err(ApiError::not_found("match"))
}

pub async fn ws(ws: WebSocketUpgrade, State(app): State<App>) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        let mut rx = app.ws_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if socket.send(Message::Text(msg.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn micro_to_usd_formats() {
        assert_eq!(super::micro_to_usd("1234567890"), "1234.567890");
        assert_eq!(super::micro_to_usd("5"), "0.000005");
        assert_eq!(super::micro_to_usd("0"), "0.000000");
        assert_eq!(super::micro_to_usd("1500000.25"), "1.500000");
    }
}
