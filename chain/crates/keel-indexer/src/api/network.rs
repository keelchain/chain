//! Validators, epochs, vaults (deposits/outbounds) and governance.
//!
//! Validator and vault state is a node snapshot (validators table, live
//! `/v1/vaults/{chain}`); deposits, outbounds, proposals and votes are
//! materialized from events, with a one-shot node fallback for objects
//! older than the indexer's first block.

use super::{jcol, jrow, page, ApiError, ApiResult, App, Cursor, Paging};
use crate::types::{field_str, field_u64};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::Row;

const CHAINS: &[&str] = &["BTC", "ETH", "TRON"];

/// `bitcoin` / `Bitcoin` / `btc` → `BTC` (the node accepts the long names).
pub fn canon_chain(s: &str) -> Option<&'static str> {
    match s.to_ascii_lowercase().as_str() {
        "btc" | "bitcoin" => Some("BTC"),
        "eth" | "ethereum" => Some("ETH"),
        "tron" | "trx" => Some("TRON"),
        _ => None,
    }
}

fn node_chain_name(c: &str) -> &'static str {
    match c {
        "BTC" => "Bitcoin",
        "ETH" => "Ethereum",
        _ => "Tron",
    }
}

// ---------------- validators / epochs ----------------

pub async fn validators(State(app): State<App>) -> ApiResult {
    let now = crate::now_ms();
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT v.address, v.consensus_key, v.self_bond::text AS self_bond, v.delegated::text AS delegated, v.power::text AS power, v.jailed, v.joined_epoch, v.in_consensus, v.epoch, v.updated_height,
                  (SELECT COUNT(*) FROM blocks b WHERE b.proposer = v.consensus_key AND b.timestamp >= $1) AS blocks_proposed_24h
           FROM validators v ORDER BY v.power DESC, v.address) x",
    )
    .bind(now - 86_400_000)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    Ok(Json(Value::Array(rows)))
}

pub async fn epochs(State(app): State<App>) -> ApiResult {
    let rows: Vec<Value> = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT epoch, start_height, COALESCE(validator_count, jsonb_array_length(validators)) AS validators, validators AS validator_set FROM epochs ORDER BY epoch DESC LIMIT 500) x")
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    Ok(Json(Value::Array(rows)))
}

// ---------------- vaults ----------------

/// Sum of balances per asset for the given chain: system vault accounts
/// (reserves) and everyone else (liabilities).
async fn chain_balances(app: &App, chain: &str) -> Result<(Vec<Value>, Vec<Value>), ApiError> {
    let rows = sqlx::query(
        "SELECT b.asset, SUM(CASE WHEN b.address = $2 THEN b.balance ELSE 0 END)::text AS reserves, SUM(CASE WHEN b.address <> $2 THEN b.balance ELSE 0 END)::text AS liabilities
         FROM balances b JOIN assets a ON a.asset = b.asset WHERE a.chain = $1 GROUP BY b.asset ORDER BY b.asset",
    )
    .bind(chain)
    .bind(crate::materialize::SYSTEM_ADDR)
    .fetch_all(&app.db.pool)
    .await?;
    let mut reserves = Vec::new();
    let mut liabilities = Vec::new();
    for r in rows {
        let asset: String = r.get("asset");
        reserves.push(json!({ "asset": asset, "amount": r.get::<String, _>("reserves") }));
        liabilities.push(json!({ "asset": asset, "amount": r.get::<String, _>("liabilities") }));
    }
    Ok((reserves, liabilities))
}

async fn vault_view(app: &App, chain: &'static str) -> Result<Option<Value>, ApiError> {
    let live = match app
        .node
        .get(&format!("/v1/vaults/{}", node_chain_name(chain)))
        .await
    {
        Ok(Some(v)) => Some(v),
        Ok(None) => None,
        Err(e) => {
            tracing::debug!(%e, chain, "vault fetch failed");
            None
        }
    };
    let (reserves, liabilities) = chain_balances(app, chain).await?;
    let deposits: i64 = sqlx::query("SELECT COUNT(*) AS n FROM deposits WHERE chain = $1")
        .bind(chain)
        .fetch_one(&app.db.pool)
        .await?
        .get("n");
    let outbounds: i64 = sqlx::query("SELECT COUNT(*) AS n FROM outbounds WHERE chain = $1")
        .bind(chain)
        .fetch_one(&app.db.pool)
        .await?
        .get("n");
    let Some(live) = live else {
        if deposits == 0 && outbounds == 0 && reserves.is_empty() {
            return Ok(None);
        }
        return Ok(Some(json!({
            "chain": chain, "epoch": null, "address_count": 0, "reserves": reserves, "liabilities": liabilities,
            "fee_rate": null, "halted": false, "halted_assets": [], "registered": false, "deposits": deposits, "outbounds": outbounds,
        })));
    };
    let vault = live.get("vault").cloned().unwrap_or(Value::Null);
    let halted: Vec<Value> = live
        .get("halted")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let address_count = field_u64(&live, "next_deposit_index")
        .map(|n| n.saturating_sub(1))
        .or_else(|| {
            live.get("deposit_owners")
                .and_then(Value::as_object)
                .map(|m| m.len() as u64)
        })
        .unwrap_or(0);
    Ok(Some(json!({
        "chain": chain,
        "epoch": vault.get("epoch").cloned(),
        "address_count": address_count,
        "reserves": reserves,
        "liabilities": liabilities,
        "fee_rate": live.get("fee_rate").cloned(),
        "halted": !halted.is_empty(),
        "halted_assets": halted,
        "registered": !vault.is_null(),
        "threshold": vault.get("threshold").cloned(),
        "signers": vault.get("signers").cloned().unwrap_or(json!([])),
        "public_key": vault.get("public_key").cloned(),
        "registered_height": vault.get("registered_height").cloned(),
        "epochs": live.get("epochs").cloned().unwrap_or(json!([])),
        "deposits": deposits,
        "outbounds": outbounds,
    })))
}

/// Lightning pools, open payouts and pending sweeps: live node state
/// (`/v1/lightning`), nothing to materialize — the pools are balances.
pub async fn lightning(State(app): State<App>) -> ApiResult {
    match app.node.get("/v1/lightning").await {
        Ok(Some(v)) => Ok(Json(v)),
        Ok(None) => Ok(Json(
            json!({ "enabled": false, "pools": [], "assignments": [], "sweeps": [], "pool_total": "0", "params": {} }),
        )),
        Err(e) => Err(ApiError::internal(format!("node /v1/lightning: {e}"))),
    }
}

pub async fn vaults(State(app): State<App>) -> ApiResult {
    let mut out = Vec::new();
    for c in CHAINS {
        if let Some(v) = vault_view(&app, c).await? {
            out.push(v);
        }
    }
    Ok(Json(Value::Array(out)))
}

#[derive(Deserialize, Default)]
pub struct StatusQuery {
    status: Option<String>,
    owner: Option<String>,
    // Not `#[serde(flatten)] Paging`: flattened numbers do not deserialize
    // from a query string.
    limit: Option<i64>,
    cursor: Option<String>,
}

impl StatusQuery {
    fn paging(&self) -> Paging {
        Paging {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

pub async fn deposits(
    State(app): State<App>,
    Path(chain): Path<String>,
    Query(q): Query<StatusQuery>,
) -> ApiResult {
    let chain = canon_chain(&chain)
        .ok_or_else(|| ApiError::bad_request("chain must be BTC, ETH or TRON"))?;
    let paging = q.paging();
    let limit = paging.limit();
    let before = Cursor::part(&paging.cursor(1)?, 0);
    let status = q.status.as_deref().map(|s| s.to_ascii_lowercase());
    let owner = q
        .owner
        .as_deref()
        .map(|o| o.trim_start_matches("0x").to_ascii_lowercase());
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT key, chain, asset, owner, amount::text AS amount, status, tx_hash, external_index AS index, deposit_index, external_height, votes, height AS first_height, updated_height AS last_height, release_height, tx_id
           FROM deposits WHERE chain = $1 AND ($2::text IS NULL OR status = $2) AND ($3::text IS NULL OR owner = $3) AND height < $4
           ORDER BY height DESC, key LIMIT $5) x",
    )
    .bind(chain)
    .bind(&status)
    .bind(&owner)
    .bind(before)
    .bind(limit + 1)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let (mut rows, next) = page(rows, limit, |d| Cursor(vec![jcol(d, "first_height")]));
    // Required depth from the chain params; current depth from the node's
    // live deposit view when it still tracks the entry.
    let params = app
        .node
        .get("/v1/params")
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let required = match chain {
        "BTC" => field_u64(&params, "confirmations_btc"),
        "ETH" => field_u64(&params, "confirmations_eth"),
        _ => field_u64(&params, "confirmations_tron"),
    };
    let live = app.node.get("/v1/vaults/deposits").await.ok().flatten();
    let live: std::collections::HashMap<String, Value> = live
        .and_then(|v| v.get("deposits").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| {
            Some((
                format!(
                    "{}:{}",
                    field_str(&d, "chain")?,
                    field_str(&d, "tx_hash")?.to_ascii_lowercase()
                ),
                d,
            ))
        })
        .collect();
    for d in &mut rows {
        d["required_depth"] = json!(required);
        let key = field_str(d, "key").unwrap_or_default();
        if let Some(l) = live.get(&key) {
            if let Some(v) = l.get("votes") {
                d["votes"] = v.clone();
            }
            if d.get("external_height").map(Value::is_null).unwrap_or(true) {
                d["external_height"] = l.get("external_height").cloned().unwrap_or(Value::Null);
            }
            d["address"] = l.get("address").cloned().unwrap_or(Value::Null);
        }
        d["depth"] = Value::Null;
    }
    Ok(Json(json!({ "deposits": rows, "next_cursor": next })))
}

pub async fn outbounds(State(app): State<App>, Query(q): Query<StatusQuery>) -> ApiResult {
    let paging = q.paging();
    let limit = paging.limit();
    let before = Cursor::part(&paging.cursor(1)?, 0);
    let status = q.status.as_deref().map(|s| s.to_ascii_lowercase());
    let owner = q
        .owner
        .as_deref()
        .map(|o| o.trim_start_matches("0x").to_ascii_lowercase());
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT id, owner, asset, chain, to_addr AS \"to\", amount::text AS amount, fee_asset, fee_estimate::text AS fee, status, batch_id, tx_hash, refunded::text AS refunded,
                  created_height AS queued_height, confirmed_height, updated_height, tx_id
           FROM outbounds WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR owner = $2) AND id < $3 ORDER BY id DESC LIMIT $4) x",
    )
    .bind(&status)
    .bind(&owner)
    .bind(before)
    .bind(limit + 1)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let (rows, next) = page(rows, limit, |o| Cursor(vec![jcol(o, "id")]));
    // Batches are not materialized from events (OutboundBatched carries the
    // outbound id only); the node keeps the full list.
    let live = app
        .node
        .get("/v1/vaults/outbounds")
        .await
        .ok()
        .flatten()
        .unwrap_or(Value::Null);
    let ids: std::collections::HashSet<i64> = rows.iter().map(|o| jcol(o, "id")).collect();
    let status_of: std::collections::HashMap<i64, String> = rows
        .iter()
        .map(|o| (jcol(o, "id"), field_str(o, "status").unwrap_or_default()))
        .collect();
    let batches: Vec<Value> = live
        .get("batches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|mut b| {
            let members: Vec<i64> = b
                .get("outbound_ids")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            if !members.iter().any(|m| ids.contains(m)) {
                return None;
            }
            let statuses: Vec<&str> = members
                .iter()
                .filter_map(|m| status_of.get(m).map(String::as_str))
                .collect();
            let status = if statuses.iter().all(|s| *s == "confirmed") && !statuses.is_empty() {
                "confirmed"
            } else if statuses.contains(&"failed") {
                "failed"
            } else if b.get("tx_hash").map(|t| !t.is_null()).unwrap_or(false) {
                "broadcast"
            } else {
                "signing"
            };
            b["status"] = json!(status);
            if b.get("tx_hash").is_none() {
                let tx = members.iter().find_map(|m| {
                    rows.iter()
                        .find(|o| jcol(o, "id") == *m)
                        .and_then(|o| o.get("tx_hash").cloned())
                        .filter(|t| !t.is_null())
                });
                b["tx_hash"] = tx.unwrap_or(Value::Null);
            }
            Some(b)
        })
        .collect();
    Ok(Json(
        json!({ "outbounds": rows, "batches": batches, "next_cursor": next }),
    ))
}

// ---------------- governance ----------------

const PROPOSAL_COLS: &str = "id, proposer, title, description, kind, status, deposit::text AS deposit, submit_height AS submitted_height, voting_end AS voting_end_height, timelock_end AS execute_height, \
    jsonb_build_object('yes', yes::text, 'no', no::text, 'abstain', abstain::text, 'veto', veto::text) AS tally, executed_ok, updated_height, tx_id";

async fn proposal_rows(app: &App, id: Option<i64>) -> Result<Vec<Value>, ApiError> {
    let rows = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {PROPOSAL_COLS} FROM proposals WHERE $1::bigint IS NULL OR id = $1 ORDER BY id DESC LIMIT 500) x"))
        .bind(id)
        .fetch_all(&app.db.pool)
        .await?;
    Ok(rows.iter().map(jrow).collect())
}

/// Live proposal records from the node (older than our first block, or the
/// tally moved without an event we materialize).
async fn enrich_from_node(app: &App, id: Option<i64>) {
    let path = match id {
        Some(id) => format!("/v1/gov/proposals/{id}"),
        None => "/v1/gov/proposals".to_string(),
    };
    if let Ok(Some(v)) = app.node.get(&path).await {
        let list: Vec<Value> = match v.get("proposals").and_then(Value::as_array) {
            Some(a) => a.clone(),
            None if v.get("id").is_some() => vec![v],
            None => vec![],
        };
        for p in &list {
            if let Err(e) = app.db.enrich_proposal(p).await {
                tracing::debug!(%e, "proposal enrich failed");
            }
        }
    }
}

pub async fn proposals(State(app): State<App>) -> ApiResult {
    enrich_from_node(&app, None).await;
    let rows = proposal_rows(&app, None).await?;
    Ok(Json(Value::Array(rows)))
}

pub async fn proposal(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("proposal id must be an integer"))?;
    enrich_from_node(&app, Some(id)).await;
    let mut p = proposal_rows(&app, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("proposal"))?;
    let votes: Vec<Value> = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT voter, choice, weight::text AS weight, height, tx_id FROM votes WHERE proposal_id = $1 ORDER BY height DESC, voter) x")
        .bind(id)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    p["votes"] = Value::Array(votes);
    Ok(Json(p))
}

/// `{budget:{base:1}, x:2}` → `{"budget.base":"1", "x":"2"}`.
pub fn flatten_params(v: &Value) -> Map<String, Value> {
    fn walk(prefix: &str, v: &Value, out: &mut Map<String, Value>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&key, x, out);
                }
            }
            Value::Null => {}
            Value::String(s) => {
                out.insert(prefix.to_string(), json!(s));
            }
            other => {
                out.insert(prefix.to_string(), json!(other.to_string()));
            }
        }
    }
    let mut out = Map::new();
    walk("", v, &mut out);
    out
}

pub async fn params(State(app): State<App>) -> ApiResult {
    let live = app.node.get("/v1/params").await.ok().flatten();
    let mut params = live.as_ref().map(flatten_params).unwrap_or_default();
    if live.is_none() {
        // Node unreachable: latest value per key from the history.
        let rows = sqlx::query("SELECT DISTINCT ON (key) key, to_value::text AS v FROM param_history ORDER BY key, height DESC, tx_index DESC, event_index DESC").fetch_all(&app.db.pool).await?;
        for r in rows {
            params.insert(r.get::<String, _>("key"), json!(r.get::<String, _>("v")));
        }
    }
    let history: Vec<Value> = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT height, timestamp, key, from_value::text AS \"from\", to_value::text AS \"to\", tx_id FROM param_history ORDER BY height DESC, tx_index DESC, event_index DESC LIMIT 500) x")
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    Ok(Json(
        json!({ "params": params, "history": history, "live": live.is_some() }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_names() {
        assert_eq!(canon_chain("Bitcoin"), Some("BTC"));
        assert_eq!(canon_chain("eth"), Some("ETH"));
        assert_eq!(canon_chain("TRX"), Some("TRON"));
        assert_eq!(canon_chain("sol"), None);
        assert_eq!(node_chain_name("TRON"), "Tron");
    }

    #[test]
    fn params_flatten() {
        let p = flatten_params(
            &json!({"budget": {"base": 10000, "max_per_block": 200}, "taker_fee_bps": 10, "name": "x", "none": null}),
        );
        assert_eq!(p["budget.base"], json!("10000"));
        assert_eq!(p["taker_fee_bps"], json!("10"));
        assert_eq!(p["name"], json!("x"));
        assert!(!p.contains_key("none"));
    }
}
