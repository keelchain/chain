//! Explorer API (`docs/explorer-api.md`). Read side over Postgres plus
//! a few live proxies to the node (balances, order book, vault state).

mod chain;
mod markets;
mod network;
mod p2p;

use crate::db::Db;
use crate::node::Node;
use crate::{Config, SyncStatus};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::postgres::PgRow;
use sqlx::Row;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct AppState {
    pub cfg: Config,
    pub db: Db,
    pub node: Node,
    pub status: Arc<SyncStatus>,
    pub ws_tx: broadcast::Sender<String>,
}

pub type App = Arc<AppState>;

pub fn router(state: App) -> Router {
    Router::new()
        .route("/v1/health", get(chain::health))
        .route("/v1/stats", get(chain::stats))
        .route("/v1/blocks", get(chain::blocks))
        .route("/v1/blocks/{height}", get(chain::block))
        .route("/v1/txs", get(chain::txs))
        .route("/v1/txs/{tx_id}", get(chain::tx))
        .route("/v1/accounts/{addr}", get(chain::account))
        .route("/v1/accounts/{addr}/txs", get(chain::account_txs))
        .route(
            "/v1/accounts/{addr}/transfers",
            get(chain::account_transfers),
        )
        .route("/v1/accounts/{addr}/orders", get(markets::account_orders))
        .route("/v1/accounts/{addr}/offers", get(p2p::account_offers))
        .route("/v1/accounts/{addr}/trades", get(p2p::account_trades))
        .route("/v1/search", get(chain::search))
        .route("/v1/ws", get(chain::ws))
        .route("/v1/assets", get(markets::assets))
        .route("/v1/assets/{asset}", get(markets::asset))
        .route("/v1/markets", get(markets::markets))
        .route("/v1/markets/{pair}", get(markets::market))
        .route("/v1/markets/{pair}/fills", get(markets::fills))
        .route("/v1/markets/{pair}/candles", get(markets::candles))
        .route("/v1/orders/{id}", get(markets::order))
        .route("/v1/offers", get(p2p::offers))
        .route("/v1/offers/{id}", get(p2p::offer))
        .route("/v1/trades/{id}", get(p2p::trade))
        .route("/v1/validators", get(network::validators))
        .route("/v1/epochs", get(network::epochs))
        .route("/v1/vaults", get(network::vaults))
        .route("/v1/vaults/outbounds", get(network::outbounds))
        .route("/v1/lightning", get(network::lightning))
        .route("/v1/vaults/{chain}/deposits", get(network::deposits))
        .route("/v1/governance/proposals", get(network::proposals))
        .route("/v1/governance/proposals/{id}", get(network::proposal))
        .route("/v1/governance/params", get(network::params))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}

// ---------------- errors ----------------

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn not_found(what: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "NOT_FOUND".into(),
            message: format!("{what} not found"),
        }
    }
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "BAD_REQUEST".into(),
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "INTERNAL".into(),
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(%e, "database error");
        ApiError::internal(format!("database: {e}"))
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(%e, "internal error");
        ApiError::internal(e.to_string())
    }
}

pub type ApiResult = Result<Json<Value>, ApiError>;

// ---------------- paging ----------------

/// Opaque list cursor: the sort key of the last row served, dot-joined
/// (`height.index` for txs, `height` for blocks, …). Lists are newest first,
/// so the next page is everything strictly below the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor(pub Vec<i64>);

impl Cursor {
    pub fn encode(&self) -> String {
        self.0
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(".")
    }

    pub fn parse(s: &str, parts: usize) -> Result<Self, ApiError> {
        let v: Result<Vec<i64>, _> = s.split('.').map(|p| p.parse::<i64>()).collect();
        match v {
            Ok(v) if v.len() == parts => Ok(Cursor(v)),
            _ => Err(ApiError::bad_request(format!(
                "cursor must be {parts} dot-separated integers"
            ))),
        }
    }

    /// The i-th component, or `i64::MAX` when there is no cursor (first page).
    pub fn part(c: &Option<Cursor>, i: usize) -> i64 {
        c.as_ref()
            .and_then(|c| c.0.get(i).copied())
            .unwrap_or(i64::MAX)
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct Paging {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

pub const DEFAULT_LIMIT: i64 = 25;
pub const MAX_LIMIT: i64 = 200;

impl Paging {
    pub fn limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
    pub fn cursor(&self, parts: usize) -> Result<Option<Cursor>, ApiError> {
        match self.cursor.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => Cursor::parse(s, parts).map(Some),
            None => Ok(None),
        }
    }
}

/// Fetch `limit + 1` rows; keep `limit` and, when the extra one exists,
/// return the cursor of the last kept row.
pub fn page<T>(
    mut rows: Vec<T>,
    limit: i64,
    key: impl Fn(&T) -> Cursor,
) -> (Vec<T>, Option<String>) {
    let limit = limit.max(1) as usize;
    if rows.len() > limit {
        rows.truncate(limit);
        let next = rows.last().map(|r| key(r).encode());
        (rows, next)
    } else {
        (rows, None)
    }
}

// ---------------- helpers ----------------

/// `SELECT to_jsonb(x) AS j …` rows.
pub fn jrow(r: &PgRow) -> Value {
    r.try_get::<Value, _>("j").unwrap_or(Value::Null)
}

pub fn jcol(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

pub fn parse_hex64(s: &str, what: &str) -> Result<String, ApiError> {
    let s = s.trim().trim_start_matches("0x").to_ascii_lowercase();
    if crate::types::is_hex64(&s) {
        Ok(s)
    } else {
        Err(ApiError::bad_request(format!(
            "{what} must be 64 hex chars"
        )))
    }
}

pub const BLOCK_COLS: &str =
    "height, timestamp, timestamp_exact, state_hash, tx_count, ok_count, event_count, proposer";
pub const TX_COLS: &str = "tx_id, height, index, timestamp, signer, nonce, module, kind, ok, CASE WHEN error_code IS NULL THEN NULL ELSE jsonb_build_object('code', error_code, 'message', error_message) END AS error, event_count";

/// Events of the given `(height, tx_index)` pairs, grouped in order.
pub async fn events_for(
    db: &Db,
    keys: &[(i64, i64)],
) -> Result<std::collections::HashMap<(i64, i64), Vec<Value>>, ApiError> {
    let mut out = std::collections::HashMap::new();
    if keys.is_empty() {
        return Ok(out);
    }
    let hs: Vec<i64> = keys.iter().map(|k| k.0).collect();
    let is: Vec<i32> = keys.iter().map(|k| k.1 as i32).collect();
    let rows = sqlx::query(
        "SELECT height, tx_index, CASE WHEN jsonb_typeof(data) = 'object' THEN jsonb_build_object('type', type) || data ELSE jsonb_build_object('type', type, 'data', data) END AS j
         FROM events WHERE height = ANY($1) AND tx_index = ANY($2) ORDER BY height, tx_index, event_index",
    )
    .bind(&hs)
    .bind(&is)
    .fetch_all(&db.pool)
    .await?;
    let wanted: std::collections::HashSet<(i64, i64)> = keys.iter().copied().collect();
    for r in rows {
        let k = (
            r.get::<i64, _>("height"),
            r.get::<i32, _>("tx_index") as i64,
        );
        if wanted.contains(&k) {
            out.entry(k).or_insert_with(Vec::new).push(jrow(&r));
        }
    }
    Ok(out)
}

/// Attach `events` to tx rows from `TX_COLS` selects.
pub async fn with_events(db: &Db, mut txs: Vec<Value>) -> Result<Vec<Value>, ApiError> {
    let keys: Vec<(i64, i64)> = txs
        .iter()
        .map(|t| (jcol(t, "height"), jcol(t, "index")))
        .collect();
    let mut ev = events_for(db, &keys).await?;
    for t in &mut txs {
        let k = (jcol(t, "height"), jcol(t, "index"));
        t["events"] = Value::Array(ev.remove(&k).unwrap_or_default());
    }
    Ok(txs)
}

/// Decimals per asset (assets table), cached per request.
pub async fn decimals_map(db: &Db) -> Result<std::collections::HashMap<String, i64>, ApiError> {
    let rows = sqlx::query("SELECT asset, decimals FROM assets")
        .fetch_all(&db.pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                r.get::<String, _>("asset"),
                r.get::<i32, _>("decimals") as i64,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip() {
        let c = Cursor(vec![42, 7, -1]);
        assert_eq!(c.encode(), "42.7.-1");
        assert_eq!(Cursor::parse("42.7.-1", 3).unwrap(), c);
        assert!(Cursor::parse("42.7", 3).is_err());
        assert!(Cursor::parse("x", 1).is_err());
        assert_eq!(Cursor::part(&None, 0), i64::MAX);
        assert_eq!(Cursor::part(&Some(c.clone()), 1), 7);
    }

    #[test]
    fn page_returns_next_cursor_only_when_more_exist() {
        let rows: Vec<i64> = (0..6).rev().collect(); // 6 rows fetched for limit 5
        let (kept, next) = page(rows, 5, |r| Cursor(vec![*r]));
        assert_eq!(kept, vec![5, 4, 3, 2, 1]);
        assert_eq!(next.as_deref(), Some("1"));
        let (kept, next) = page(vec![3, 2, 1], 5, |r| Cursor(vec![*r]));
        assert_eq!(kept.len(), 3);
        assert!(next.is_none());
        let (kept, next) = page(Vec::<i64>::new(), 5, |r| Cursor(vec![*r]));
        assert!(kept.is_empty() && next.is_none());
    }

    #[test]
    fn paging_limits_are_clamped() {
        let p = Paging {
            limit: Some(10_000),
            cursor: Some("9.1".into()),
        };
        assert_eq!(p.limit(), MAX_LIMIT);
        assert_eq!(p.cursor(2).unwrap(), Some(Cursor(vec![9, 1])));
        assert!(p.cursor(1).is_err());
        assert_eq!(
            Paging {
                limit: Some(0),
                cursor: Some(String::new())
            }
            .limit(),
            1
        );
        assert_eq!(Paging::default().cursor(2).unwrap(), None);
    }
}
