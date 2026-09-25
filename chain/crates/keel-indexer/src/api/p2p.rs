//! P2P marketplace: offers, trades, and the per-account lists.
//!
//! Rows come from Postgres (materialized from events, enriched from the
//! node's records); a miss falls back to the node's live view once so an
//! object created before the indexer's first block still resolves.

use super::{jcol, jrow, page, ApiError, ApiResult, App, Cursor, Paging};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

const OFFER_COLS: &str = "id, owner, side, asset, fiat_currency, payment_method, margin_bps, fixed_price::text AS fixed_price, min_amount::text AS min_amount, max_amount::text AS max_amount, payment_window_secs, country, min_tier, \
    CASE WHEN status = 'open' THEN 'active' ELSE status END AS status, created_height, updated_height, tx_id";
const TRADE_COLS: &str = "id, offer_id, buyer, seller, asset, amount::text AS amount, fee::text AS fee, fiat_amount::text AS fiat_amount, fiat_currency, status, started_height, started_at, deadline, paid_at, closed_height, updated_height, tx_id, NULLIF(dispute, '{}'::jsonb) AS dispute, history";

#[derive(Deserialize, Default)]
pub struct OfferQuery {
    asset: Option<String>,
    side: Option<String>,
    status: Option<String>,
    owner: Option<String>,
    // Not `#[serde(flatten)] Paging`: flattened numbers do not deserialize
    // from a query string.
    limit: Option<i64>,
    cursor: Option<String>,
}

impl OfferQuery {
    fn paging(&self) -> Paging {
        Paging {
            limit: self.limit,
            cursor: self.cursor.clone(),
        }
    }
}

/// Explorer status names → stored names (`active` is stored as `open`).
fn stored_status(s: &str) -> String {
    match s.to_ascii_lowercase().as_str() {
        "active" | "open" | "live" => "open".into(),
        other => other.to_string(),
    }
}

pub async fn offers(State(app): State<App>, Query(q): Query<OfferQuery>) -> ApiResult {
    let paging = q.paging();
    let limit = paging.limit();
    let before = Cursor::part(&paging.cursor(1)?, 0);
    let side = q.side.as_deref().map(|s| s.to_ascii_lowercase());
    let status = q.status.as_deref().map(stored_status);
    let owner = q
        .owner
        .as_deref()
        .map(|o| o.trim_start_matches("0x").to_ascii_lowercase());
    let rows: Vec<Value> = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {OFFER_COLS} FROM offers WHERE id < $1
           AND ($2::text IS NULL OR asset = $2) AND ($3::text IS NULL OR side = $3) AND ($4::text IS NULL OR status = $4) AND ($5::text IS NULL OR owner = $5)
         ORDER BY id DESC LIMIT $6) x"
    ))
    .bind(before)
    .bind(&q.asset)
    .bind(&side)
    .bind(&status)
    .bind(&owner)
    .bind(limit + 1)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let (rows, next) = page(rows, limit, |o| Cursor(vec![jcol(o, "id")]));
    Ok(Json(json!({ "offers": rows, "next_cursor": next })))
}

async fn offer_row(app: &App, id: i64) -> Result<Option<Value>, ApiError> {
    let row = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {OFFER_COLS} FROM offers WHERE id = $1) x"
    ))
    .bind(id)
    .fetch_optional(&app.db.pool)
    .await?;
    Ok(row.map(|r| jrow(&r)))
}

pub async fn offer(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("offer id must be an integer"))?;
    let mut o = match offer_row(&app, id).await? {
        Some(o) => o,
        None => {
            // Created before our first indexed block: take the node's record.
            if let Ok(Some(v)) = app.node.get(&format!("/v1/offers/{id}")).await {
                app.db.enrich_offer(&v).await?;
            }
            offer_row(&app, id)
                .await?
                .ok_or_else(|| ApiError::not_found("offer"))?
        }
    };
    let trades: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {TRADE_COLS} FROM trades WHERE offer_id = $1 ORDER BY id DESC LIMIT 200) x"))
        .bind(id)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    o["trades"] = Value::Array(trades);
    Ok(Json(o))
}

async fn trade_row(app: &App, id: i64) -> Result<Option<Value>, ApiError> {
    let row = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {TRADE_COLS} FROM trades WHERE id = $1) x"
    ))
    .bind(id)
    .fetch_optional(&app.db.pool)
    .await?;
    Ok(row.map(|r| jrow(&r)))
}

pub async fn trade(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("trade id must be an integer"))?;
    let t = match trade_row(&app, id).await? {
        Some(t) => t,
        None => {
            if let Ok(Some(v)) = app.node.get(&format!("/v1/trades/{id}")).await {
                app.db.enrich_trade(&v).await?;
            }
            trade_row(&app, id)
                .await?
                .ok_or_else(|| ApiError::not_found("trade"))?
        }
    };
    Ok(Json(t))
}

pub async fn account_offers(
    State(app): State<App>,
    Path(addr): Path<String>,
    Query(p): Query<Paging>,
) -> ApiResult {
    let addr = super::parse_hex64(&addr, "address")?;
    let limit = p.limit();
    let before = Cursor::part(&p.cursor(1)?, 0);
    let rows: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {OFFER_COLS} FROM offers WHERE owner = $1 AND id < $2 ORDER BY id DESC LIMIT $3) x"))
        .bind(&addr)
        .bind(before)
        .bind(limit + 1)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    let (rows, next) = page(rows, limit, |o| Cursor(vec![jcol(o, "id")]));
    Ok(Json(json!({ "offers": rows, "next_cursor": next })))
}

pub async fn account_trades(
    State(app): State<App>,
    Path(addr): Path<String>,
    Query(p): Query<Paging>,
) -> ApiResult {
    let addr = super::parse_hex64(&addr, "address")?;
    let limit = p.limit();
    let before = Cursor::part(&p.cursor(1)?, 0);
    let rows: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {TRADE_COLS} FROM trades WHERE (buyer = $1 OR seller = $1) AND id < $2 ORDER BY id DESC LIMIT $3) x"))
        .bind(&addr)
        .bind(before)
        .bind(limit + 1)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    let (rows, next) = page(rows, limit, |t| Cursor(vec![jcol(t, "id")]));
    Ok(Json(json!({ "trades": rows, "next_cursor": next })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_aliases() {
        assert_eq!(super::stored_status("active"), "open");
        assert_eq!(super::stored_status("Open"), "open");
        assert_eq!(super::stored_status("PAUSED"), "paused");
        assert_eq!(super::stored_status("closed"), "closed");
    }
}
