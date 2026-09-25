//! Assets, markets (with live book from the node), fills, candles, orders.

use super::{jcol, jrow, page, ApiError, ApiResult, App, Cursor, Paging};
use crate::types::{field_str, field_u64};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

const ASSET_COLS: &str =
    "asset, decimals, kind, chain, supply::text AS supply, holders, updated_at";

pub async fn assets(State(app): State<App>) -> ApiResult {
    let rows: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {ASSET_COLS}, (SELECT COALESCE(SUM(b.balance), 0)::text FROM balances b WHERE b.asset = a.asset AND b.address = $1 AND b.account_type IN ('vault_asset', 'stable_reserve')) AS reserves FROM assets a ORDER BY asset) x"))
        .bind(crate::materialize::SYSTEM_ADDR)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    Ok(Json(Value::Array(rows)))
}

pub async fn asset(State(app): State<App>, Path(asset): Path<String>) -> ApiResult {
    let row = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {ASSET_COLS} FROM assets WHERE asset = $1) x"
    ))
    .bind(&asset)
    .fetch_optional(&app.db.pool)
    .await?;
    let mut a = row
        .map(|r| jrow(&r))
        .ok_or_else(|| ApiError::not_found("asset"))?;
    let holders: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (SELECT address, SUM(balance)::text AS balance FROM balances WHERE asset = $1 AND address <> $2 AND balance > 0 GROUP BY address ORDER BY SUM(balance) DESC LIMIT 20) x",
    )
    .bind(&asset)
    .bind(crate::materialize::SYSTEM_ADDR)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let now = crate::now_ms();
    let t24 = sqlx::query("SELECT COUNT(*) AS n, COALESCE(SUM(amount), 0)::text AS volume FROM transfers WHERE asset = $1 AND timestamp >= $2").bind(&asset).bind(now - 86_400_000).fetch_one(&app.db.pool).await?;
    let reserves = sqlx::query("SELECT COALESCE(SUM(balance), 0)::text AS r FROM balances WHERE asset = $1 AND address = $2 AND account_type IN ('vault_asset', 'stable_reserve')")
        .bind(&asset)
        .bind(crate::materialize::SYSTEM_ADDR)
        .fetch_one(&app.db.pool)
        .await?
        .get::<String, _>("r");
    a["holders_top"] = Value::Array(holders);
    a["transfers_24h"] = json!(t24.get::<i64, _>("n"));
    a["volume_24h"] = json!(t24.get::<String, _>("volume"));
    a["reserves"] = json!(reserves);
    let markets: Vec<Value> = sqlx::query("SELECT to_jsonb(x) AS j FROM (SELECT pair FROM markets WHERE base = $1 OR quote = $1 ORDER BY pair) x").bind(&asset).fetch_all(&app.db.pool).await?.iter().map(|r| jrow(r)["pair"].clone()).collect();
    a["markets"] = Value::Array(markets);
    Ok(Json(a))
}

/// Indexed market rows with 24h aggregates, merged with the node's live
/// summary (best bid/ask) when reachable.
async fn market_rows(app: &App, pair: Option<&str>) -> Result<Vec<Value>, ApiError> {
    let now = crate::now_ms();
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT m.pair, m.base, m.quote, m.base_decimals, m.quote_decimals, m.last_price::text AS last_price, m.cfg,
                  COALESCE((SELECT SUM(quantity) FROM fills f WHERE f.pair = m.pair AND f.timestamp >= $1), 0)::text AS volume_24h_base,
                  COALESCE((SELECT SUM(quote) FROM fills f WHERE f.pair = m.pair AND f.timestamp >= $1), 0)::text AS volume_24h_quote,
                  (SELECT COUNT(*) FROM fills f WHERE f.pair = m.pair AND f.timestamp >= $1) AS trades_24h,
                  (SELECT price::text FROM fills f WHERE f.pair = m.pair AND f.timestamp < $1 ORDER BY height DESC, tx_index DESC, event_index DESC LIMIT 1) AS price_24h_ago
           FROM markets m WHERE $2::text IS NULL OR m.pair = $2 ORDER BY m.pair) x",
    )
    .bind(now - 86_400_000)
    .bind(pair)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let live = app.node.get("/v1/markets").await.ok().flatten();
    let live: std::collections::HashMap<String, Value> = live
        .and_then(|v| v.get("markets").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| field_str(&m, "symbol").map(|s| (s, m)))
        .collect();
    Ok(rows
        .into_iter()
        .map(|mut r| {
            let pair = field_str(&r, "pair").unwrap_or_default();
            let l = live.get(&pair);
            r["best_bid"] = l
                .and_then(|l| l.get("best_bid").cloned())
                .unwrap_or(Value::Null);
            r["best_ask"] = l
                .and_then(|l| l.get("best_ask").cloned())
                .unwrap_or(Value::Null);
            r["open_orders"] = l
                .and_then(|l| l.get("open_orders").cloned())
                .unwrap_or(Value::Null);
            r["house_quote"] = l
                .and_then(|l| l.get("house").cloned())
                .unwrap_or(Value::Null);
            if let Some(lp) = l.and_then(|l| l.get("last_price")).filter(|v| !v.is_null()) {
                r["last_price"] = json!(crate::types::amount_of(lp).map(|a| a.to_string()));
            }
            r["enabled"] = r["cfg"].get("enabled").cloned().unwrap_or(json!(true));
            r
        })
        .collect())
}

pub async fn markets(State(app): State<App>) -> ApiResult {
    Ok(Json(Value::Array(market_rows(&app, None).await?)))
}

#[derive(Deserialize)]
pub struct DepthQuery {
    depth: Option<u32>,
}

pub async fn market(
    State(app): State<App>,
    Path(pair): Path<String>,
    Query(q): Query<DepthQuery>,
) -> ApiResult {
    let mut m = market_rows(&app, Some(&pair))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found("pair"))?;
    let depth = q.depth.unwrap_or(50).clamp(1, 200);
    let book = app
        .node
        .get(&format!("/v1/markets/{pair}/book?depth={depth}"))
        .await
        .ok()
        .flatten();
    let levels = |side: &str| -> Vec<Value> {
        book.as_ref()
            .and_then(|b| b.get(side))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|l| json!([field_str(l, "price"), field_str(l, "size")]))
                    .collect()
            })
            .unwrap_or_default()
    };
    m["book"] = json!({ "bids": levels("bids"), "asks": levels("asks"), "height": book.as_ref().and_then(|b| field_u64(b, "height")), "live": book.is_some() });
    if let Some(h) = book.as_ref().and_then(|b| b.get("house")) {
        m["house_quote"] = h.clone();
    }
    Ok(Json(m))
}

pub async fn fills(
    State(app): State<App>,
    Path(pair): Path<String>,
    Query(p): Query<Paging>,
) -> ApiResult {
    let limit = p.limit();
    let c = p.cursor(3)?;
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT height, tx_index, event_index, tx_id, timestamp, pair, price::text AS price, quantity::text AS quantity, quote::text AS quote, fee::text AS fee, fee_asset, taker, taker_side, taker_order_id, maker_order_id, maker
           FROM fills WHERE pair = $1 AND (height, tx_index, event_index) < ($2, $3, $4)
           ORDER BY height DESC, tx_index DESC, event_index DESC LIMIT $5) x",
    )
    .bind(&pair)
    .bind(Cursor::part(&c, 0))
    .bind(Cursor::part(&c, 1) as i32)
    .bind(Cursor::part(&c, 2) as i32)
    .bind(limit + 1)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    let (rows, next) = page(rows, limit, |f| {
        Cursor(vec![
            jcol(f, "height"),
            jcol(f, "tx_index"),
            jcol(f, "event_index"),
        ])
    });
    Ok(Json(json!({ "fills": rows, "next_cursor": next })))
}

#[derive(Deserialize)]
pub struct CandleQuery {
    interval: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    limit: Option<i64>,
}

pub fn interval_ms(s: &str) -> Option<i64> {
    match s {
        "1m" => Some(60_000),
        "5m" => Some(300_000),
        "15m" => Some(900_000),
        "1h" => Some(3_600_000),
        "4h" => Some(14_400_000),
        "1d" => Some(86_400_000),
        _ => None,
    }
}

pub async fn candles(
    State(app): State<App>,
    Path(pair): Path<String>,
    Query(q): Query<CandleQuery>,
) -> ApiResult {
    let iv = interval_ms(q.interval.as_deref().unwrap_or("1m"))
        .ok_or_else(|| ApiError::bad_request("interval must be one of 1m, 5m, 15m, 1h, 4h, 1d"))?;
    let limit = q.limit.unwrap_or(500).clamp(1, 5000);
    let to = q.to.unwrap_or_else(crate::now_ms);
    let from = q.from.unwrap_or(to - iv * limit);
    let rows: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (
           SELECT (bucket / $2) * $2 AS t, (array_agg(o ORDER BY bucket))[1]::text AS o, MAX(h)::text AS h, MIN(l)::text AS l,
                  (array_agg(c ORDER BY bucket DESC))[1]::text AS c, SUM(v)::text AS v, SUM(qv)::text AS qv, SUM(n) AS n
           FROM candles WHERE pair = $1 AND bucket >= $3 AND bucket < $4 GROUP BY 1 ORDER BY 1 LIMIT $5) x",
    )
    .bind(&pair)
    .bind(iv)
    .bind(from)
    .bind(to)
    .bind(limit)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    Ok(Json(Value::Array(rows)))
}

const ORDER_COLS: &str = "id, owner, pair, side, order_type, price::text AS price, quantity::text AS quantity, quote_budget::text AS quote_budget, client_id, resting::text AS resting, filled::text AS filled, filled_quote::text AS filled_quote, released::text AS released, status, created_height, updated_height, tx_id";

pub async fn account_orders(
    State(app): State<App>,
    Path(addr): Path<String>,
    Query(p): Query<Paging>,
) -> ApiResult {
    let addr = super::parse_hex64(&addr, "address")?;
    let limit = p.limit();
    let before = Cursor::part(&p.cursor(1)?, 0);
    let rows: Vec<Value> = sqlx::query(&format!("SELECT to_jsonb(x) AS j FROM (SELECT {ORDER_COLS} FROM orders WHERE owner = $1 AND id < $2 ORDER BY id DESC LIMIT $3) x"))
        .bind(&addr)
        .bind(before)
        .bind(limit + 1)
        .fetch_all(&app.db.pool)
        .await?
        .iter()
        .map(jrow)
        .collect();
    let (rows, next) = page(rows, limit, |o| Cursor(vec![jcol(o, "id")]));
    Ok(Json(json!({ "orders": rows, "next_cursor": next })))
}

pub async fn order(State(app): State<App>, Path(id): Path<String>) -> ApiResult {
    let id: i64 = id
        .parse()
        .map_err(|_| ApiError::bad_request("order id must be an integer"))?;
    let row = sqlx::query(&format!(
        "SELECT to_jsonb(x) AS j FROM (SELECT {ORDER_COLS} FROM orders WHERE id = $1) x"
    ))
    .bind(id)
    .fetch_optional(&app.db.pool)
    .await?;
    let mut o = row
        .map(|r| jrow(&r))
        .ok_or_else(|| ApiError::not_found("order"))?;
    let fills: Vec<Value> = sqlx::query(
        "SELECT to_jsonb(x) AS j FROM (SELECT height, tx_index, event_index, tx_id, timestamp, pair, price::text AS price, quantity::text AS quantity, quote::text AS quote, fee::text AS fee, fee_asset, taker, taker_side, taker_order_id, maker_order_id, maker,
           CASE WHEN taker_order_id = $1 THEN 'taker' ELSE 'maker' END AS role FROM fills WHERE taker_order_id = $1 OR maker_order_id = $1 ORDER BY height DESC, tx_index DESC, event_index DESC LIMIT 500) x",
    )
    .bind(id)
    .fetch_all(&app.db.pool)
    .await?
    .iter()
    .map(jrow)
    .collect();
    o["fills"] = Value::Array(fills);
    Ok(Json(o))
}

#[cfg(test)]
mod tests {
    #[test]
    fn intervals() {
        assert_eq!(super::interval_ms("5m"), Some(300_000));
        assert_eq!(super::interval_ms("1d"), Some(86_400_000));
        assert_eq!(super::interval_ms("2w"), None);
    }
}
