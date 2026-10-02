//! The testnet faucet: `GET /v1/faucet` describes it, `POST /v1/faucet`
//! sends a fixed amount of KEEL and KUSD to an address, once per address
//! per cooldown and a few times per IP per day. The faucet key is an
//! ordinary account funded from genesis; every send is a plain `Transfer`
//! the explorer shows like any other, and every claim is a row here.

use super::{ApiError, ApiResult, App};
use axum::extract::{ConnectInfo, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use keel_actions::{Action, SignedAction, Transfer};
use keel_crypto::Keypair;
use keel_types::{Address, Asset};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;

const DECIMALS: u128 = 1_000_000;

#[derive(Deserialize)]
pub struct ClaimBody {
    pub address: String,
}

#[derive(Deserialize)]
pub struct ClaimsQuery {
    pub address: Option<String>,
    pub limit: Option<i64>,
}

/// Pure limiter: given the seconds since this address's last claim and the
/// IP's claims today, why a claim is refused (None = allowed).
pub fn refusal(
    since_last_for_address: Option<i64>,
    ip_claims_today: i64,
    cooldown_secs: i64,
    ip_per_day: i64,
) -> Option<String> {
    if let Some(s) = since_last_for_address {
        if s < cooldown_secs {
            let left = cooldown_secs - s;
            let h = left / 3600;
            let m = (left % 3600) / 60;
            return Some(format!(
                "this address claimed {}h {}m ago; try again in {}h {}m",
                s / 3600,
                (s % 3600) / 60,
                h,
                m
            ));
        }
    }
    if ip_claims_today >= ip_per_day {
        return Some(format!(
            "this network address reached {ip_per_day} claims today; come back tomorrow"
        ));
    }
    None
}

fn client_ip(headers: &HeaderMap, peer: SocketAddr) -> String {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    forwarded
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_else(|| peer.ip().to_string())
}

fn faucet(app: &App) -> Result<&crate::FaucetConfig, ApiError> {
    app.cfg.faucet.as_ref().ok_or_else(|| ApiError {
        status: axum::http::StatusCode::NOT_FOUND,
        code: "FAUCET_OFF".into(),
        message: "this network has no faucet configured".into(),
    })
}

fn explorer_tx(cfg: &crate::FaucetConfig, tx: &str) -> String {
    format!("{}/tx/{tx}", cfg.explorer_url.trim_end_matches('/'))
}

/// What the faucet gives, who it is, and what it still holds.
pub async fn info(State(app): State<App>) -> ApiResult {
    let cfg = faucet(&app)?;
    let key = Keypair::from_secret(cfg.secret);
    let address = key.address().to_hex();
    let account = app.node.get(&format!("/v1/accounts/{address}")).await.ok().flatten();
    let balance_of = |asset: &str| -> String {
        account
            .as_ref()
            .and_then(|a| a.get("balances"))
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter(|r| r["asset"] == asset && r["account_type"] == "deposit")
                    .filter_map(|r| r["balance"].as_str().and_then(|b| b.parse::<u128>().ok()))
                    .sum::<u128>()
            })
            .map(|units| format!("{}", units / DECIMALS))
            .unwrap_or_else(|| "0".into())
    };
    let claims_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM faucet_claims WHERE asset = 'KEEL' AND claimed_at > now() - interval '1 day'",
    )
    .fetch_one(&app.db.pool)
    .await?;
    Ok(Json(json!({
        "enabled": true,
        "address": address,
        "amounts": { "KEEL": cfg.keel.to_string(), "KUSD": cfg.kusd.to_string() },
        "cooldown_secs": cfg.cooldown_secs,
        "ip_per_day": cfg.ip_per_day,
        "balances": { "KEEL": balance_of("KEEL"), "KUSD": balance_of("KUSD") },
        "claims_today": claims_today,
        "explorer_url": cfg.explorer_url,
    })))
}

/// Recent claims, optionally one address's.
pub async fn claims(State(app): State<App>, Query(q): Query<ClaimsQuery>) -> ApiResult {
    faucet(&app)?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    let rows: Vec<(String, String, String, String, chrono_free::Stamp)> = match q.address.as_deref() {
        Some(a) => {
            let a = a.trim().to_ascii_lowercase();
            sqlx::query_as(
                "SELECT address, asset, amount::text, tx_id, claimed_at FROM faucet_claims WHERE address = $1 ORDER BY claimed_at DESC LIMIT $2",
            )
            .bind(a)
            .bind(limit)
            .fetch_all(&app.db.pool)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT address, asset, amount::text, tx_id, claimed_at FROM faucet_claims ORDER BY claimed_at DESC LIMIT $1",
            )
            .bind(limit)
            .fetch_all(&app.db.pool)
            .await?
        }
    };
    let cfg = faucet(&app)?;
    Ok(Json(json!({
        "claims": rows.iter().map(|(address, asset, amount, tx_id, at)| json!({
            "address": address,
            "asset": asset,
            "amount": amount.parse::<u128>().map(|u| (u / DECIMALS).to_string()).unwrap_or_else(|_| amount.clone()),
            "tx_id": tx_id,
            "url": explorer_tx(cfg, tx_id),
            "claimed_at": at.0,
        })).collect::<Vec<_>>()
    })))
}

/// Send KEEL and KUSD to `address`, within the limits.
pub async fn claim(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<ClaimBody>,
) -> ApiResult {
    let cfg = faucet(&app)?.clone();
    let address = body.address.trim().to_ascii_lowercase();
    let to = Address::from_hex(&address)
        .ok_or_else(|| ApiError::bad_request("address must be 64 hex characters (a Keel account address)"))?;
    let key = Keypair::from_secret(cfg.secret);
    if to == key.address() {
        return Err(ApiError::bad_request("that is the faucet's own address"));
    }
    let ip = client_ip(&headers, peer);

    // One claim at a time: the nonce is read from the node and the two
    // transfers go out back to back.
    let _guard = app.faucet_lock.lock().await;
    let since_last: Option<i64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (now() - MAX(claimed_at)))::bigint FROM faucet_claims WHERE address = $1",
    )
    .bind(&address)
    .fetch_one(&app.db.pool)
    .await?;
    let ip_today: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT tx_id) FROM faucet_claims WHERE ip = $1 AND asset = 'KEEL' AND claimed_at > now() - interval '1 day'",
    )
    .bind(&ip)
    .fetch_one(&app.db.pool)
    .await?;
    if let Some(why) = refusal(since_last, ip_today, cfg.cooldown_secs, cfg.ip_per_day) {
        return Err(ApiError {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            code: "RATE_LIMITED".into(),
            message: why,
        });
    }

    let me = key.address().to_hex();
    let status = app
        .node
        .get("/v1/status")
        .await?
        .ok_or_else(|| ApiError::internal("node status unavailable"))?;
    let chain_id = status["chain_id"].as_u64().unwrap_or(0) as u32;
    let account = app
        .node
        .get(&format!("/v1/accounts/{me}"))
        .await?
        .unwrap_or(Value::Null);
    let mut nonce = account["nonce"]
        .as_u64()
        .or_else(|| account["nonce"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);

    let mut sent = Vec::new();
    for (asset, coins) in [("KEEL", cfg.keel), ("KUSD", cfg.kusd)] {
        if coins == 0 {
            continue;
        }
        let amount = coins as u128 * DECIMALS;
        let action = Action::Transfer(Transfer {
            to,
            asset: Asset::new(asset),
            amount,
            memo: Some("faucet".into()),
        });
        let signed = SignedAction::sign(&key, nonce, chain_id, action);
        let answer = app.node.post("/v1/actions", &serde_json::to_value(&signed).map_err(|e| ApiError::internal(e.to_string()))?).await?;
        if answer["admitted"] != true {
            let why = answer["error"].as_str().unwrap_or("refused").to_string();
            tracing::warn!(%why, %asset, "faucet transfer refused");
            return Err(ApiError {
                status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
                code: "FAUCET_EMPTY".into(),
                message: format!("the faucet could not send {asset}: {why}"),
            });
        }
        let tx_id = answer["tx_id"].as_str().unwrap_or_default().to_string();
        sqlx::query("INSERT INTO faucet_claims (address, ip, asset, amount, tx_id) VALUES ($1, $2, $3, $4::numeric, $5)")
            .bind(&address)
            .bind(&ip)
            .bind(asset)
            .bind(amount.to_string())
            .bind(&tx_id)
            .execute(&app.db.pool)
            .await?;
        sent.push(json!({ "asset": asset, "amount": coins.to_string(), "tx_id": tx_id, "url": explorer_tx(&cfg, &tx_id) }));
        nonce += 1;
    }
    Ok(Json(json!({ "address": address, "sent": sent, "next_claim_in_secs": cfg.cooldown_secs })))
}

/// A `TIMESTAMPTZ` as RFC 3339 without pulling a date crate into the API.
mod chrono_free {
    use sqlx::postgres::PgValueRef;
    use sqlx::{Decode, Postgres, Type};

    pub struct Stamp(pub String);

    impl Type<Postgres> for Stamp {
        fn type_info() -> sqlx::postgres::PgTypeInfo {
            <String as Type<Postgres>>::type_info()
        }
        fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
            ty.to_string() == "TIMESTAMPTZ" || <String as Type<Postgres>>::compatible(ty)
        }
    }

    impl<'r> Decode<'r, Postgres> for Stamp {
        fn decode(value: PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
            // Text protocol: the server renders the stamp; binary needs the
            // epoch microseconds since 2000-01-01.
            match value.format() {
                sqlx::postgres::PgValueFormat::Text => Ok(Stamp(value.as_str()?.to_string())),
                sqlx::postgres::PgValueFormat::Binary => {
                    let micros = i64::from_be_bytes(value.as_bytes()?.try_into()?);
                    let secs = micros.div_euclid(1_000_000) + 946_684_800;
                    Ok(Stamp(epoch_to_rfc3339(secs)))
                }
            }
        }
    }

    /// Civil date from a Unix timestamp (Howard Hinnant's algorithm).
    fn epoch_to_rfc3339(secs: i64) -> String {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::refusal;

    #[test]
    fn limiter_names_the_reason() {
        assert_eq!(refusal(None, 0, 86_400, 5), None);
        assert_eq!(refusal(Some(90_000), 0, 86_400, 5), None);
        let why = refusal(Some(3_600), 0, 86_400, 5).unwrap();
        assert!(why.contains("try again in 23h"), "{why}");
        let why = refusal(None, 5, 86_400, 5).unwrap();
        assert!(why.contains("5 claims today"), "{why}");
    }
}
