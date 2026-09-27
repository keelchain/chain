//! Postgres access. Writes go through [`Db::write_batch`]: one transaction per
//! block range, bulk `UNNEST` inserts, and every derived row keyed by
//! (height, index) so re-running a block is a no-op (block rows are inserted
//! with `ON CONFLICT DO NOTHING RETURNING height`, and only newly inserted
//! heights have their rollups applied).

use crate::materialize::{
    Candle, Ctx, CtxRefs, Materialized, OfferInfo, OrderInfo, OutboundInfo, Patch, Touched,
    TradeInfo,
};
use crate::types::{amount_of, field_amount, field_str, field_u64, GENESIS_ASSETS};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

#[derive(Clone, Debug, Default)]
pub struct SyncState {
    pub chain_id: Option<i64>,
    pub indexed_height: i64,
    pub first_indexed_height: Option<i64>,
    pub last_state_hash: Option<String>,
    pub hash_mismatch: Option<(i64, String, String)>,
}

pub struct WriteOutcome {
    pub new_heights: usize,
    pub last_height: Option<i64>,
    pub touched: Touched,
}

/// Asset-side (debit-normal) accounts from `keel_ledger::catalog`: the
/// reserves and mint counters. Supply is the credit side only, so these are
/// excluded from the per-asset sum whoever holds them (the network vault's
/// reserve sits under the system address, a client's under its own).
const DEBIT_NORMAL_SYSTEM_ACCOUNTS: &[&str] = &[
    "issuance",
    "vault_asset",
    "hot_wallet",
    "warm_wallet",
    "cold_wallet",
    "fuel_tank",
    "deposit_addresses",
    "deposit_incoming",
    "sweep_gas",
];

fn s(a: u128) -> String {
    a.to_string()
}

fn os(a: Option<u128>) -> Option<String> {
    a.map(|x| x.to_string())
}

impl Db {
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(16)
            .connect(url)
            .await
            .context("connect postgres")?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .context("run migrations")?;
        Ok(())
    }

    // ---------------- sync state ----------------

    pub async fn load_sync_state(&self, network: &str) -> Result<SyncState> {
        let row = sqlx::query("SELECT chain_id, indexed_height, first_indexed_height, last_state_hash, hash_mismatch_height, hash_mismatch_node, hash_mismatch_stored FROM sync_state WHERE network = $1")
            .bind(network)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => SyncState {
                chain_id: r.try_get("chain_id")?,
                indexed_height: r.try_get("indexed_height")?,
                first_indexed_height: r.try_get("first_indexed_height")?,
                last_state_hash: r.try_get("last_state_hash")?,
                hash_mismatch: match (
                    r.try_get::<Option<i64>, _>("hash_mismatch_height")?,
                    r.try_get::<Option<String>, _>("hash_mismatch_node")?,
                    r.try_get::<Option<String>, _>("hash_mismatch_stored")?,
                ) {
                    (Some(h), Some(n), Some(st)) => Some((h, n, st)),
                    _ => None,
                },
            },
            None => {
                sqlx::query("INSERT INTO sync_state (network) VALUES ($1) ON CONFLICT DO NOTHING")
                    .bind(network)
                    .execute(&self.pool)
                    .await?;
                SyncState::default()
            }
        })
    }

    pub async fn set_chain_id(&self, network: &str, chain_id: i64) -> Result<()> {
        sqlx::query("UPDATE sync_state SET chain_id = $2, updated_at = now() WHERE network = $1")
            .bind(network)
            .bind(chain_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record_hash_mismatch(
        &self,
        network: &str,
        height: i64,
        node: &str,
        stored: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE sync_state SET hash_mismatch_height = $2, hash_mismatch_node = $3, hash_mismatch_stored = $4, updated_at = now() WHERE network = $1")
            .bind(network)
            .bind(height)
            .bind(node)
            .bind(stored)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Timestamp of the highest block below `height` (carry-forward source).
    pub async fn timestamp_before(&self, height: i64) -> Result<Option<i64>> {
        let r = sqlx::query(
            "SELECT timestamp FROM blocks WHERE height < $1 ORDER BY height DESC LIMIT 1",
        )
        .bind(height)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|x| x.get::<i64, _>("timestamp")))
    }

    pub async fn block_state_hash(&self, height: i64) -> Result<Option<Option<String>>> {
        let r = sqlx::query("SELECT state_hash FROM blocks WHERE height = $1")
            .bind(height)
            .fetch_optional(&self.pool)
            .await?;
        Ok(r.map(|x| x.get::<Option<String>, _>("state_hash")))
    }

    /// Heights in `[from, to]` with no block row.
    pub async fn missing_heights(&self, from: i64, to: i64) -> Result<Vec<i64>> {
        let rows = sqlx::query("SELECT g.h FROM generate_series($1::bigint, $2::bigint) AS g(h) LEFT JOIN blocks b ON b.height = g.h WHERE b.height IS NULL ORDER BY g.h")
            .bind(from)
            .bind(to)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.iter().map(|r| r.get::<i64, _>("h")).collect())
    }

    // ---------------- context preload ----------------

    pub async fn preload_ctx(&self, refs: &CtxRefs) -> Result<Ctx> {
        let mut ctx = Ctx::default();
        let rows = sqlx::query("SELECT pair, base, quote FROM markets")
            .fetch_all(&self.pool)
            .await?;
        for r in rows {
            ctx.markets
                .insert(r.get("pair"), (r.get("base"), r.get("quote")));
        }
        if !refs.orders.is_empty() {
            let ids: Vec<i64> = refs.orders.iter().map(|x| *x as i64).collect();
            for r in sqlx::query("SELECT id, owner, pair, side FROM orders WHERE id = ANY($1)")
                .bind(&ids)
                .fetch_all(&self.pool)
                .await?
            {
                ctx.orders.insert(
                    r.get::<i64, _>("id") as u64,
                    OrderInfo {
                        owner: r.get("owner"),
                        pair: r.get("pair"),
                        side: r.get("side"),
                    },
                );
            }
        }
        if !refs.outbounds.is_empty() {
            let ids: Vec<i64> = refs.outbounds.iter().map(|x| *x as i64).collect();
            for r in sqlx::query(
                "SELECT id, owner, asset, amount::text AS amount FROM outbounds WHERE id = ANY($1)",
            )
            .bind(&ids)
            .fetch_all(&self.pool)
            .await?
            {
                ctx.outbounds.insert(
                    r.get::<i64, _>("id") as u64,
                    OutboundInfo {
                        owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
                        asset: r.get::<Option<String>, _>("asset").unwrap_or_default(),
                        amount: r
                            .get::<Option<String>, _>("amount")
                            .and_then(|a| a.parse().ok())
                            .unwrap_or(0),
                    },
                );
            }
        }
        if !refs.trades.is_empty() {
            let ids: Vec<i64> = refs.trades.iter().map(|x| *x as i64).collect();
            for r in sqlx::query("SELECT id, buyer, seller, asset, amount::text AS amount FROM trades WHERE id = ANY($1)").bind(&ids).fetch_all(&self.pool).await? {
                ctx.trades.insert(
                    r.get::<i64, _>("id") as u64,
                    TradeInfo {
                        buyer: r.get::<Option<String>, _>("buyer").unwrap_or_default(),
                        seller: r.get::<Option<String>, _>("seller").unwrap_or_default(),
                        asset: r.get::<Option<String>, _>("asset").unwrap_or_default(),
                        amount: r.get::<Option<String>, _>("amount").and_then(|a| a.parse().ok()).unwrap_or(0),
                    },
                );
            }
        }
        if !refs.offers.is_empty() {
            let ids: Vec<i64> = refs.offers.iter().map(|x| *x as i64).collect();
            for r in
                sqlx::query("SELECT id, owner, asset, fiat_currency FROM offers WHERE id = ANY($1)")
                    .bind(&ids)
                    .fetch_all(&self.pool)
                    .await?
            {
                ctx.offers.insert(
                    r.get::<i64, _>("id") as u64,
                    OfferInfo {
                        owner: r.get("owner"),
                        asset: r.get::<Option<String>, _>("asset").unwrap_or_default(),
                        fiat_currency: r.get("fiat_currency"),
                    },
                );
            }
        }
        if refs.held {
            for r in sqlx::query("SELECT key, owner, asset, amount::text AS amount FROM deposits WHERE status = 'held'").fetch_all(&self.pool).await? {
                let amount: u128 = r.get::<Option<String>, _>("amount").and_then(|a| a.parse().ok()).unwrap_or(0);
                ctx.held.insert((r.get::<Option<String>, _>("owner").unwrap_or_default(), r.get::<Option<String>, _>("asset").unwrap_or_default(), amount), r.get("key"));
            }
        }
        if !refs.params.is_empty() {
            let keys: Vec<String> = refs.params.iter().cloned().collect();
            for r in sqlx::query("SELECT DISTINCT ON (key) key, to_value::text AS v FROM param_history WHERE key = ANY($1) ORDER BY key, height DESC, tx_index DESC, event_index DESC").bind(&keys).fetch_all(&self.pool).await? {
                if let Ok(v) = r.get::<String, _>("v").parse::<u128>() {
                    ctx.params.insert(r.get("key"), v);
                }
            }
        }
        Ok(ctx)
    }

    /// Seed `ctx.params` for keys never changed on chain from the node's
    /// current params, so the first change records a `from` value.
    pub fn seed_params(ctx: &mut Ctx, params: &Value) {
        let Some(obj) = params.as_object() else {
            return;
        };
        for (k, v) in obj {
            if let Some(a) = amount_of(v) {
                ctx.params.entry(k.clone()).or_insert(a);
            } else if let Some(sub) = v.as_object() {
                for (k2, v2) in sub {
                    if let Some(a) = amount_of(v2) {
                        ctx.params.entry(format!("{k}.{k2}")).or_insert(a);
                    }
                }
            }
        }
    }

    // ---------------- batch write ----------------

    pub async fn write_batch(
        &self,
        network: &str,
        mats: Vec<Materialized>,
    ) -> Result<WriteOutcome> {
        if mats.is_empty() {
            return Ok(WriteOutcome {
                new_heights: 0,
                last_height: None,
                touched: Touched::default(),
            });
        }
        let mut tx = self.pool.begin().await?;
        // 1. blocks (only new heights get their derived rows)
        let (mut h, mut ts, mut ex, mut sh, mut tc, mut oc, mut ec, mut pr) = (
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        for m in &mats {
            let b = &m.block;
            h.push(b.height);
            ts.push(b.timestamp);
            ex.push(b.timestamp_exact);
            sh.push(b.state_hash.clone());
            tc.push(b.tx_count);
            oc.push(b.ok_count);
            ec.push(b.event_count);
            pr.push(b.proposer.clone());
        }
        let inserted = sqlx::query(
            "INSERT INTO blocks (height, timestamp, timestamp_exact, state_hash, tx_count, ok_count, event_count, proposer)
             SELECT * FROM UNNEST($1::bigint[], $2::bigint[], $3::bool[], $4::text[], $5::int[], $6::int[], $7::int[], $8::text[])
             ON CONFLICT (height) DO NOTHING RETURNING height",
        )
        .bind(&h)
        .bind(&ts)
        .bind(&ex)
        .bind(&sh)
        .bind(&tc)
        .bind(&oc)
        .bind(&ec)
        .bind(&pr)
        .fetch_all(&mut *tx)
        .await
        .context("insert blocks")?;
        let new: HashSet<i64> = inserted.iter().map(|r| r.get::<i64, _>("height")).collect();
        let last_height = h.iter().copied().max();
        let mats: Vec<Materialized> = mats
            .into_iter()
            .filter(|m| new.contains(&m.block.height))
            .collect();
        let mut touched = Touched::default();
        if !mats.is_empty() {
            insert_txs(&mut tx, &mats).await?;
            insert_events(&mut tx, &mats).await?;
            insert_transfers(&mut tx, &mats).await?;
            insert_fills(&mut tx, &mats).await?;
            upsert_candles(&mut tx, &mats).await?;
            upsert_accounts(&mut tx, &mats).await?;
            for m in &mats {
                touched.merge(&m.touched);
                for p in &m.patches {
                    apply_patch(&mut tx, p).await?;
                }
            }
        }
        if let Some(last) = last_height {
            let first = h.iter().copied().min().unwrap_or(last);
            sqlx::query(
                "UPDATE sync_state SET indexed_height = GREATEST(indexed_height, $2), first_indexed_height = LEAST(COALESCE(first_indexed_height, $3), $3), updated_at = now() WHERE network = $1",
            )
            .bind(network)
            .bind(last)
            .bind(first)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await.context("commit batch")?;
        Ok(WriteOutcome {
            new_heights: mats.len(),
            last_height,
            touched,
        })
    }

    // ---------------- node snapshots ----------------

    pub async fn upsert_markets(&self, markets: &Value, now: i64) -> Result<()> {
        let list = markets
            .get("markets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for m in list {
            let cfg = m.get("cfg").cloned().unwrap_or(Value::Null);
            let Some(pair) = field_str(&m, "symbol").or_else(|| field_str(&cfg, "symbol")) else {
                continue;
            };
            let base = field_str(&cfg, "base_asset").unwrap_or_default();
            let quote = field_str(&cfg, "quote_asset").unwrap_or_default();
            let bd = field_u64(&cfg, "base_decimals").unwrap_or(0) as i32;
            let qd = field_u64(&cfg, "quote_decimals").unwrap_or(0) as i32;
            sqlx::query(
                "INSERT INTO markets (pair, base, quote, base_decimals, quote_decimals, cfg, last_price, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7::text::numeric, $8)
                 ON CONFLICT (pair) DO UPDATE SET base = EXCLUDED.base, quote = EXCLUDED.quote, base_decimals = EXCLUDED.base_decimals,
                   quote_decimals = EXCLUDED.quote_decimals, cfg = EXCLUDED.cfg, last_price = COALESCE(EXCLUDED.last_price, markets.last_price), updated_at = EXCLUDED.updated_at",
            )
            .bind(&pair)
            .bind(&base)
            .bind(&quote)
            .bind(bd)
            .bind(qd)
            .bind(&cfg)
            .bind(os(field_amount(&m, "last_price")))
            .bind(now)
            .execute(&self.pool)
            .await?;
            for (asset, dec) in [(&base, bd), (&quote, qd)] {
                if !asset.is_empty() {
                    self.upsert_asset(asset, dec, None, now).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn upsert_asset(
        &self,
        asset: &str,
        decimals: i32,
        kind: Option<&str>,
        now: i64,
    ) -> Result<()> {
        let chain = crate::types::asset_chain(asset).map(str::to_string);
        let kind = kind.map(str::to_string).unwrap_or_else(|| {
            if chain.is_some() {
                "vault".into()
            } else {
                "native".into()
            }
        });
        sqlx::query(
            "INSERT INTO assets (asset, decimals, kind, chain, updated_at) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (asset) DO UPDATE SET decimals = EXCLUDED.decimals, chain = EXCLUDED.chain, kind = CASE WHEN assets.kind = 'unknown' THEN EXCLUDED.kind ELSE assets.kind END",
        )
        .bind(asset)
        .bind(decimals)
        .bind(kind)
        .bind(chain)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn seed_assets(&self, now: i64) -> Result<()> {
        for (asset, dec, kind) in GENESIS_ASSETS {
            let chain = crate::types::asset_chain(asset);
            sqlx::query("INSERT INTO assets (asset, decimals, kind, chain, updated_at) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (asset) DO NOTHING")
                .bind(asset)
                .bind(*dec as i32)
                .bind(kind)
                .bind(chain)
                .bind(now)
                .execute(&self.pool)
                .await?;
        }
        // RegisterAsset proposals that executed.
        let rows = sqlx::query(
            "SELECT kind FROM proposals WHERE status = 'executed' AND kind ? 'RegisterAsset'",
        )
        .fetch_all(&self.pool)
        .await?;
        for r in rows {
            let k: Value = r.get("kind");
            if let (Some(asset), Some(dec)) = (
                field_str(&k["RegisterAsset"], "asset"),
                field_u64(&k["RegisterAsset"], "decimals"),
            ) {
                self.upsert_asset(&asset, dec as i32, None, now).await?;
            }
        }
        Ok(())
    }

    /// Replace one address's balances with the node's view.
    pub async fn replace_balances(
        &self,
        address: &str,
        balances: &[Value],
        now: i64,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM balances WHERE address = $1")
            .bind(address)
            .execute(&mut *tx)
            .await?;
        let (mut asset, mut typ, mut bal) = (vec![], vec![], vec![]);
        for b in balances {
            if let (Some(a), Some(t), Some(v)) = (
                field_str(b, "asset"),
                field_str(b, "account_type"),
                field_amount(b, "balance"),
            ) {
                asset.push(a);
                typ.push(t);
                bal.push(s(v));
            }
        }
        if !asset.is_empty() {
            sqlx::query(
                "INSERT INTO balances (address, asset, account_type, balance, updated_at)
                 SELECT $1, u.asset, u.typ, u.bal::numeric, $5 FROM UNNEST($2::text[], $3::text[], $4::text[]) AS u(asset, typ, bal)
                 ON CONFLICT (address, asset, account_type) DO UPDATE SET balance = EXCLUDED.balance, updated_at = EXCLUDED.updated_at",
            )
            .bind(address)
            .bind(&asset)
            .bind(&typ)
            .bind(&bal)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Supply and holder counts from the reconciled balances.
    pub async fn refresh_asset_stats(&self, now: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO assets (asset, decimals, kind, chain, updated_at)
             SELECT DISTINCT b.asset, 0, 'unknown', split_part(b.asset, '.', 1) NULLIF(split_part(b.asset, '.', 2), ''), $1 FROM balances b
             ON CONFLICT (asset) DO NOTHING",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .ok();
        sqlx::query(
            "UPDATE assets a SET supply = st.supply, holders = st.holders, updated_at = $2 FROM (
               SELECT asset, COALESCE(SUM(balance) FILTER (WHERE NOT (account_type = ANY($3))), 0) AS supply,
                      COUNT(DISTINCT address) FILTER (WHERE address <> $1 AND balance > 0) AS holders
               FROM balances GROUP BY asset) st WHERE st.asset = a.asset",
        )
        .bind(crate::materialize::SYSTEM_ADDR)
        .bind(now)
        .bind(DEBIT_NORMAL_SYSTEM_ACCOUNTS)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn addresses(&self, since_height: Option<i64>) -> Result<Vec<String>> {
        let rows = match since_height {
            Some(h) => {
                sqlx::query("SELECT address FROM account_stats WHERE last_seen_height >= $1")
                    .bind(h)
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("SELECT address FROM account_stats")
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        Ok(rows.iter().map(|r| r.get("address")).collect())
    }

    /// `/v1/staking/validators` snapshot → validators + epochs.
    pub async fn upsert_validators(&self, v: &Value, height: i64, now: i64) -> Result<()> {
        let consensus: HashSet<String> = v
            .get("consensus")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_ascii_lowercase()))
                    .collect()
            })
            .unwrap_or_default();
        let power: HashMap<String, String> = v
            .get("staked")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| {
                        Some((
                            field_str(x, "consensus_key")?.to_ascii_lowercase(),
                            s(field_amount(x, "power")?),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let staking = v.get("staking").cloned().unwrap_or(Value::Null);
        let epoch = field_u64(&staking, "epoch").map(|e| e as i64);
        let mut seen = Vec::new();
        let mut keys = Vec::new();
        if let Some(vals) = staking.get("validators").and_then(Value::as_object) {
            for (addr, rec) in vals {
                let addr = addr.to_ascii_lowercase();
                let key = rec
                    .get("consensus_key")
                    .and_then(|k| {
                        crate::types::address_of(k)
                            .or_else(|| k.as_str().map(|x| x.to_ascii_lowercase()))
                    })
                    .unwrap_or_default();
                let self_bond = s(field_amount(rec, "self_bond").unwrap_or(0));
                let delegated = s(field_amount(rec, "delegated").unwrap_or(0));
                let p = power.get(&key).cloned().unwrap_or_else(|| {
                    let sb: u128 = self_bond.parse().unwrap_or(0);
                    let d: u128 = delegated.parse().unwrap_or(0);
                    s(sb.saturating_add(d))
                });
                sqlx::query(
                    "INSERT INTO validators (address, consensus_key, self_bond, delegated, power, jailed, joined_epoch, in_consensus, epoch, updated_height, updated_at)
                     VALUES ($1, $2, $3::text::numeric, $4::text::numeric, $5::text::numeric, $6, $7, $8, $9, $10, $11)
                     ON CONFLICT (address) DO UPDATE SET consensus_key = EXCLUDED.consensus_key, self_bond = EXCLUDED.self_bond, delegated = EXCLUDED.delegated,
                       power = EXCLUDED.power, jailed = EXCLUDED.jailed, joined_epoch = EXCLUDED.joined_epoch, in_consensus = EXCLUDED.in_consensus,
                       epoch = EXCLUDED.epoch, updated_height = EXCLUDED.updated_height, updated_at = EXCLUDED.updated_at",
                )
                .bind(&addr)
                .bind(&key)
                .bind(&self_bond)
                .bind(&delegated)
                .bind(&p)
                .bind(rec.get("jailed").and_then(Value::as_bool).unwrap_or(false))
                .bind(field_u64(rec, "joined_epoch").map(|e| e as i64))
                .bind(consensus.contains(&key))
                .bind(epoch)
                .bind(height)
                .bind(now)
                .execute(&self.pool)
                .await?;
                seen.push(addr.clone());
                keys.push(json!({"address": addr, "consensus_key": key}));
            }
        }
        if !seen.is_empty() {
            sqlx::query("DELETE FROM validators WHERE NOT (address = ANY($1))")
                .bind(&seen)
                .execute(&self.pool)
                .await?;
        }
        if let Some(e) = epoch {
            sqlx::query(
                "INSERT INTO epochs (epoch, start_height, validator_count, validators) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (epoch) DO UPDATE SET validators = EXCLUDED.validators, validator_count = EXCLUDED.validator_count",
            )
            .bind(e)
            .bind(height)
            .bind(keys.len() as i32)
            .bind(Value::Array(keys))
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    // ---------------- enrichment from node records ----------------

    pub async fn enrich_order(&self, v: &Value) -> Result<()> {
        let Some(id) = field_u64(v, "id") else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO orders (id, owner, pair, side, order_type, price, quantity, quote_budget, client_id, resting, filled, status, created_height, updated_height)
             VALUES ($1, $2, $3, $4, $5, $6::text::numeric, $7::text::numeric, $8::text::numeric, $9, $10::text::numeric, COALESCE($11::text::numeric, 0), $12, $13, $13)
             ON CONFLICT (id) DO UPDATE SET side = COALESCE(EXCLUDED.side, orders.side), order_type = COALESCE(EXCLUDED.order_type, orders.order_type),
               price = COALESCE(EXCLUDED.price, orders.price), quantity = COALESCE(EXCLUDED.quantity, orders.quantity), quote_budget = COALESCE(EXCLUDED.quote_budget, orders.quote_budget),
               client_id = COALESCE(EXCLUDED.client_id, orders.client_id), resting = COALESCE(EXCLUDED.resting, orders.resting),
               filled = GREATEST(orders.filled, EXCLUDED.filled), status = EXCLUDED.status",
        )
        .bind(id as i64)
        .bind(crate::types::field_addr(v, "owner").unwrap_or_default())
        .bind(field_str(v, "pair").unwrap_or_default())
        .bind(v.get("side").and_then(Value::as_str).map(|x| x.to_ascii_lowercase()))
        .bind(v.get("order_type").and_then(Value::as_str).map(|x| x.to_ascii_lowercase()))
        .bind(os(field_amount(v, "price")))
        .bind(os(field_amount(v, "quantity")))
        .bind(os(field_amount(v, "quote_budget")))
        .bind(field_u64(v, "client_id").map(|x| x as i64))
        .bind(os(field_amount(v, "remaining")))
        .bind(os(field_amount(v, "filled")))
        .bind(snake(v.get("status")))
        .bind(field_u64(v, "created_height").unwrap_or(0) as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn enrich_offer(&self, v: &Value) -> Result<()> {
        let Some(id) = field_u64(v, "id") else {
            return Ok(());
        };
        let spec = v.get("spec").cloned().unwrap_or(Value::Null);
        let status = if v.get("closed").and_then(Value::as_bool).unwrap_or(false) {
            "closed"
        } else if v.get("paused").and_then(Value::as_bool).unwrap_or(false) {
            "paused"
        } else {
            "open"
        };
        let created = field_u64(v, "created_height").unwrap_or(0) as i64;
        let p = crate::materialize::OfferPatch {
            id: id as i64,
            owner: crate::types::field_addr(v, "owner"),
            spec: Some(spec),
            status: Some(status.into()),
            created_height: Some(created),
            updated_height: created,
            tx_id: None,
        };
        let mut tx = self.pool.begin().await?;
        apply_offer(&mut tx, &p).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn enrich_trade(&self, v: &Value) -> Result<()> {
        let Some(id) = field_u64(v, "id") else {
            return Ok(());
        };
        let dispute = v.get("dispute").filter(|d| !d.is_null()).cloned();
        sqlx::query(
            "INSERT INTO trades (id, offer_id, buyer, seller, asset, amount, fee, fiat_amount, fiat_currency, status, started_at, deadline, paid_at, updated_height, dispute)
             VALUES ($1, $2, $3, $4, $5, $6::text::numeric, $7::text::numeric, $8::text::numeric, $9, $10, $11, $12, $13, 0, $14)
             ON CONFLICT (id) DO UPDATE SET offer_id = COALESCE(trades.offer_id, EXCLUDED.offer_id), buyer = COALESCE(trades.buyer, EXCLUDED.buyer), seller = COALESCE(trades.seller, EXCLUDED.seller),
               asset = COALESCE(EXCLUDED.asset, trades.asset), amount = COALESCE(trades.amount, EXCLUDED.amount), fee = COALESCE(EXCLUDED.fee, trades.fee),
               fiat_amount = COALESCE(EXCLUDED.fiat_amount, trades.fiat_amount), fiat_currency = COALESCE(EXCLUDED.fiat_currency, trades.fiat_currency),
               status = EXCLUDED.status, started_at = COALESCE(EXCLUDED.started_at, trades.started_at), deadline = COALESCE(EXCLUDED.deadline, trades.deadline),
               paid_at = COALESCE(EXCLUDED.paid_at, trades.paid_at), dispute = COALESCE(trades.dispute, '{}'::jsonb) || COALESCE(EXCLUDED.dispute, '{}'::jsonb)",
        )
        .bind(id as i64)
        .bind(field_u64(v, "offer_id").map(|x| x as i64))
        .bind(crate::types::field_addr(v, "buyer"))
        .bind(crate::types::field_addr(v, "seller"))
        .bind(field_str(v, "asset"))
        .bind(os(field_amount(v, "amount")))
        .bind(os(field_amount(v, "fee")))
        .bind(os(field_amount(v, "fiat_amount")))
        .bind(field_str(v, "fiat_currency"))
        .bind(snake(v.get("status")))
        .bind(field_u64(v, "started_at").map(|x| x as i64))
        .bind(field_u64(v, "deadline").map(|x| x as i64))
        .bind(field_u64(v, "paid_at").map(|x| x as i64))
        .bind(dispute.map(|d| dispute_json(&d)))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn enrich_proposal(&self, v: &Value) -> Result<()> {
        let Some(id) = field_u64(v, "id") else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO proposals (id, proposer, title, description, kind, status, deposit, submit_height, voting_end, timelock_end, yes, no, abstain, veto, updated_height)
             VALUES ($1, $2, $3, $4, $5, $6, $7::text::numeric, $8, $9, $10, COALESCE($11::text::numeric, 0), COALESCE($12::text::numeric, 0), COALESCE($13::text::numeric, 0), COALESCE($14::text::numeric, 0), COALESCE($8, 0))
             ON CONFLICT (id) DO UPDATE SET proposer = COALESCE(EXCLUDED.proposer, proposals.proposer), title = COALESCE(EXCLUDED.title, proposals.title), description = COALESCE(EXCLUDED.description, proposals.description),
               kind = COALESCE(EXCLUDED.kind, proposals.kind), status = EXCLUDED.status, deposit = COALESCE(EXCLUDED.deposit, proposals.deposit), submit_height = COALESCE(EXCLUDED.submit_height, proposals.submit_height),
               voting_end = COALESCE(EXCLUDED.voting_end, proposals.voting_end), timelock_end = COALESCE(EXCLUDED.timelock_end, proposals.timelock_end),
               yes = EXCLUDED.yes, no = EXCLUDED.no, abstain = EXCLUDED.abstain, veto = EXCLUDED.veto",
        )
        .bind(id as i64)
        .bind(crate::types::field_addr(v, "proposer"))
        .bind(field_str(v, "title"))
        .bind(field_str(v, "description"))
        .bind(v.get("kind").cloned())
        .bind(snake(v.get("status")))
        .bind(os(field_amount(v, "deposit")))
        .bind(field_u64(v, "submit_height").map(|x| x as i64))
        .bind(field_u64(v, "voting_end").map(|x| x as i64))
        .bind(field_u64(v, "timelock_end").map(|x| x as i64))
        .bind(os(field_amount(v, "yes")))
        .bind(os(field_amount(v, "no")))
        .bind(os(field_amount(v, "abstain")))
        .bind(os(field_amount(v, "veto")))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn enrich_outbound(&self, v: &Value) -> Result<()> {
        let Some(id) = field_u64(v, "id") else {
            return Ok(());
        };
        sqlx::query(
            "INSERT INTO outbounds (id, owner, asset, chain, to_addr, amount, fee_asset, fee_estimate, status, batch_id, tx_hash, created_height, updated_height)
             VALUES ($1, $2, $3, $4, $5, $6::text::numeric, $7, $8::text::numeric, $9, $10, $11, $12, COALESCE($12, 0))
             ON CONFLICT (id) DO UPDATE SET owner = COALESCE(outbounds.owner, EXCLUDED.owner), asset = COALESCE(outbounds.asset, EXCLUDED.asset), chain = COALESCE(EXCLUDED.chain, outbounds.chain),
               to_addr = COALESCE(EXCLUDED.to_addr, outbounds.to_addr), amount = COALESCE(outbounds.amount, EXCLUDED.amount), fee_asset = COALESCE(EXCLUDED.fee_asset, outbounds.fee_asset),
               fee_estimate = COALESCE(EXCLUDED.fee_estimate, outbounds.fee_estimate), status = EXCLUDED.status, batch_id = COALESCE(EXCLUDED.batch_id, outbounds.batch_id),
               tx_hash = COALESCE(EXCLUDED.tx_hash, outbounds.tx_hash), created_height = COALESCE(outbounds.created_height, EXCLUDED.created_height)",
        )
        .bind(id as i64)
        .bind(crate::types::field_addr(v, "owner"))
        .bind(field_str(v, "asset"))
        .bind(field_str(v, "chain"))
        .bind(field_str(v, "to"))
        .bind(os(field_amount(v, "amount")))
        .bind(field_str(v, "fee_asset"))
        .bind(os(field_amount(v, "fee_estimate")))
        .bind(snake(v.get("status")))
        .bind(field_u64(v, "batch_id").map(|x| x as i64))
        .bind(field_str(v, "tx_hash").map(|x| x.to_ascii_lowercase()))
        .bind(field_u64(v, "created_height").map(|x| x as i64))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// `"PartiallyFilled"` → `partially_filled`; enum variants are serialized as
/// bare strings by the VM and as `{:?}` strings by the RPC views.
pub fn snake(v: Option<&Value>) -> String {
    let raw = match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    };
    let mut out = String::new();
    for (i, ch) in raw.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn dispute_json(d: &Value) -> Value {
    json!({
        "opened_by": crate::types::field_addr(d, "opened_by"),
        "opened_at": field_u64(d, "opened_at"),
        "evidence": d.get("evidence").cloned().unwrap_or(json!([])),
        "ruling": d.get("ruling").cloned(),
        "ruled_by": crate::types::field_addr(d, "ruled_by"),
        "ruled_at": field_u64(d, "ruled_at"),
    })
}

// ---------------- bulk inserts ----------------

async fn insert_txs(tx: &mut Transaction<'_, Postgres>, mats: &[Materialized]) -> Result<()> {
    let rows: Vec<_> = mats.iter().flat_map(|m| m.txs.iter()).collect();
    if rows.is_empty() {
        return Ok(());
    }
    let (
        mut h,
        mut i,
        mut id,
        mut ts,
        mut sg,
        mut nn,
        mut md,
        mut kd,
        mut ac,
        mut ok,
        mut ecd,
        mut emg,
        mut ec,
    ) = (
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    for r in rows {
        h.push(r.height);
        i.push(r.index);
        id.push(r.tx_id.clone());
        ts.push(r.timestamp);
        sg.push(r.signer.clone());
        nn.push(r.nonce);
        md.push(r.module.clone());
        kd.push(r.kind.clone());
        ac.push(r.action.clone().unwrap_or(Value::Null));
        ok.push(r.ok);
        ecd.push(r.error_code.clone());
        emg.push(r.error_message.clone());
        ec.push(r.event_count);
    }
    sqlx::query(
        "INSERT INTO txs (height, index, tx_id, timestamp, signer, nonce, module, kind, action, ok, error_code, error_message, event_count)
         SELECT u.h, u.i, u.id, u.ts, u.sg, u.nn, u.md, u.kd, NULLIF(u.ac, 'null'::jsonb), u.ok, u.ecd, u.emg, u.ec
         FROM UNNEST($1::bigint[], $2::int[], $3::text[], $4::bigint[], $5::text[], $6::bigint[], $7::text[], $8::text[], $9::jsonb[], $10::bool[], $11::text[], $12::text[], $13::int[])
           AS u(h, i, id, ts, sg, nn, md, kd, ac, ok, ecd, emg, ec)
         ON CONFLICT (height, index) DO NOTHING",
    )
    .bind(&h)
    .bind(&i)
    .bind(&id)
    .bind(&ts)
    .bind(&sg)
    .bind(&nn)
    .bind(&md)
    .bind(&kd)
    .bind(&ac)
    .bind(&ok)
    .bind(&ecd)
    .bind(&emg)
    .bind(&ec)
    .execute(&mut **tx)
    .await
    .context("insert txs")?;
    Ok(())
}

async fn insert_events(tx: &mut Transaction<'_, Postgres>, mats: &[Materialized]) -> Result<()> {
    let rows: Vec<_> = mats.iter().flat_map(|m| m.events.iter()).collect();
    if rows.is_empty() {
        return Ok(());
    }
    let (mut h, mut ti, mut ei, mut id, mut ts, mut ty, mut dt, mut ad) = (
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    for r in rows {
        h.push(r.height);
        ti.push(r.tx_index);
        ei.push(r.event_index);
        id.push(r.tx_id.clone());
        ts.push(r.timestamp);
        ty.push(r.kind.clone());
        dt.push(r.data.clone());
        ad.push(r.addresses.join(","));
    }
    sqlx::query(
        "INSERT INTO events (height, tx_index, event_index, tx_id, timestamp, type, data, addresses)
         SELECT u.h, u.ti, u.ei, u.id, u.ts, u.ty, u.dt, CASE WHEN u.ad = '' THEN '{}'::text[] ELSE string_to_array(u.ad, ',') END
         FROM UNNEST($1::bigint[], $2::int[], $3::int[], $4::text[], $5::bigint[], $6::text[], $7::jsonb[], $8::text[]) AS u(h, ti, ei, id, ts, ty, dt, ad)
         ON CONFLICT (height, tx_index, event_index) DO NOTHING",
    )
    .bind(&h)
    .bind(&ti)
    .bind(&ei)
    .bind(&id)
    .bind(&ts)
    .bind(&ty)
    .bind(&dt)
    .bind(&ad)
    .execute(&mut **tx)
    .await
    .context("insert events")?;
    Ok(())
}

async fn insert_transfers(tx: &mut Transaction<'_, Postgres>, mats: &[Materialized]) -> Result<()> {
    let rows: Vec<_> = mats.iter().flat_map(|m| m.transfers.iter()).collect();
    if rows.is_empty() {
        return Ok(());
    }
    let (mut h, mut ti, mut ei, mut lg, mut id, mut ts, mut asset, mut amt, mut fr, mut to, mut kd) = (
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    for r in rows {
        h.push(r.height);
        ti.push(r.tx_index);
        ei.push(r.event_index);
        lg.push(r.leg);
        id.push(r.tx_id.clone());
        ts.push(r.timestamp);
        asset.push(r.asset.clone());
        amt.push(s(r.amount));
        fr.push(r.from.clone());
        to.push(r.to.clone());
        kd.push(r.kind.clone());
    }
    sqlx::query(
        "INSERT INTO transfers (height, tx_index, event_index, leg, tx_id, timestamp, asset, amount, from_addr, to_addr, kind)
         SELECT u.h, u.ti, u.ei, u.lg, u.id, u.ts, u.asset, u.amt::numeric, u.fr, u.t, u.kd
         FROM UNNEST($1::bigint[], $2::int[], $3::int[], $4::smallint[], $5::text[], $6::bigint[], $7::text[], $8::text[], $9::text[], $10::text[], $11::text[]) AS u(h, ti, ei, lg, id, ts, asset, amt, fr, t, kd)
         ON CONFLICT (height, tx_index, event_index, leg) DO NOTHING",
    )
    .bind(&h)
    .bind(&ti)
    .bind(&ei)
    .bind(&lg)
    .bind(&id)
    .bind(&ts)
    .bind(&asset)
    .bind(&amt)
    .bind(&fr)
    .bind(&to)
    .bind(&kd)
    .execute(&mut **tx)
    .await
    .context("insert transfers")?;
    Ok(())
}

async fn insert_fills(tx: &mut Transaction<'_, Postgres>, mats: &[Materialized]) -> Result<()> {
    let rows: Vec<_> = mats.iter().flat_map(|m| m.fills.iter()).collect();
    if rows.is_empty() {
        return Ok(());
    }
    let (
        mut h,
        mut ti,
        mut ei,
        mut id,
        mut ts,
        mut pair,
        mut px,
        mut qty,
        mut qt,
        mut fee,
        mut fa,
        mut tk,
        mut sd,
        mut toid,
        mut moid,
        mut mk,
    ) = (
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    );
    for r in rows {
        h.push(r.height);
        ti.push(r.tx_index);
        ei.push(r.event_index);
        id.push(r.tx_id.clone());
        ts.push(r.timestamp);
        pair.push(r.pair.clone());
        px.push(s(r.price));
        qty.push(s(r.quantity));
        qt.push(s(r.quote));
        fee.push(s(r.fee));
        fa.push(r.fee_asset.clone());
        tk.push(r.taker.clone());
        sd.push(r.taker_side.clone());
        toid.push(r.taker_order_id);
        moid.push(r.maker_order_id);
        mk.push(r.maker.clone());
    }
    sqlx::query(
        "INSERT INTO fills (height, tx_index, event_index, tx_id, timestamp, pair, price, quantity, quote, fee, fee_asset, taker, taker_side, taker_order_id, maker_order_id, maker)
         SELECT u.h, u.ti, u.ei, u.id, u.ts, u.pair, u.px::numeric, u.qty::numeric, u.qt::numeric, u.fee::numeric, u.fa, u.tk, u.sd, u.toid, u.moid, u.mk
         FROM UNNEST($1::bigint[], $2::int[], $3::int[], $4::text[], $5::bigint[], $6::text[], $7::text[], $8::text[], $9::text[], $10::text[], $11::text[], $12::text[], $13::text[], $14::bigint[], $15::bigint[], $16::text[])
           AS u(h, ti, ei, id, ts, pair, px, qty, qt, fee, fa, tk, sd, toid, moid, mk)
         ON CONFLICT (height, tx_index, event_index) DO NOTHING",
    )
    .bind(&h)
    .bind(&ti)
    .bind(&ei)
    .bind(&id)
    .bind(&ts)
    .bind(&pair)
    .bind(&px)
    .bind(&qty)
    .bind(&qt)
    .bind(&fee)
    .bind(&fa)
    .bind(&tk)
    .bind(&sd)
    .bind(&toid)
    .bind(&moid)
    .bind(&mk)
    .execute(&mut **tx)
    .await
    .context("insert fills")?;
    // Market last price follows the newest fill.
    for m in mats {
        if let Some(f) = m.fills.last() {
            sqlx::query("UPDATE markets SET last_price = $2::text::numeric WHERE pair = $1")
                .bind(&f.pair)
                .bind(s(f.price))
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

async fn upsert_candles(tx: &mut Transaction<'_, Postgres>, mats: &[Materialized]) -> Result<()> {
    let mut merged: BTreeMap<(String, i64), Candle> = BTreeMap::new();
    for m in mats {
        for (k, c) in &m.candles {
            merged.entry(k.clone()).or_default().merge(c);
        }
    }
    for ((pair, bucket), c) in merged {
        sqlx::query(
            "INSERT INTO candles (pair, bucket, o, h, l, c, v, qv, n) VALUES ($1, $2, $3::text::numeric, $4::text::numeric, $5::text::numeric, $6::text::numeric, $7::text::numeric, $8::text::numeric, $9)
             ON CONFLICT (pair, bucket) DO UPDATE SET h = GREATEST(candles.h, EXCLUDED.h), l = LEAST(candles.l, EXCLUDED.l), c = EXCLUDED.c,
               v = candles.v + EXCLUDED.v, qv = candles.qv + EXCLUDED.qv, n = candles.n + EXCLUDED.n",
        )
        .bind(&pair)
        .bind(bucket)
        .bind(s(c.o))
        .bind(s(c.h))
        .bind(s(c.l))
        .bind(s(c.c))
        .bind(s(c.v))
        .bind(s(c.qv))
        .bind(c.n)
        .execute(&mut **tx)
        .await
        .context("upsert candles")?;
    }
    Ok(())
}

async fn upsert_accounts(tx: &mut Transaction<'_, Postgres>, mats: &[Materialized]) -> Result<()> {
    let mut merged: BTreeMap<String, (i64, i64, i64)> = BTreeMap::new();
    for m in mats {
        for (a, d) in &m.accounts {
            let e = merged
                .entry(a.clone())
                .or_insert((d.first_seen, d.last_seen, 0));
            e.0 = e.0.min(d.first_seen);
            e.1 = e.1.max(d.last_seen);
            e.2 += d.tx_count;
        }
    }
    if merged.is_empty() {
        return Ok(());
    }
    let (mut a, mut f, mut l, mut c) = (vec![], vec![], vec![], vec![]);
    for (addr, (first, last, count)) in merged {
        a.push(addr);
        f.push(first);
        l.push(last);
        c.push(count);
    }
    sqlx::query(
        "INSERT INTO account_stats (address, first_seen_height, last_seen_height, tx_count)
         SELECT * FROM UNNEST($1::text[], $2::bigint[], $3::bigint[], $4::bigint[])
         ON CONFLICT (address) DO UPDATE SET first_seen_height = LEAST(account_stats.first_seen_height, EXCLUDED.first_seen_height),
           last_seen_height = GREATEST(account_stats.last_seen_height, EXCLUDED.last_seen_height), tx_count = account_stats.tx_count + EXCLUDED.tx_count",
    )
    .bind(&a)
    .bind(&f)
    .bind(&l)
    .bind(&c)
    .execute(&mut **tx)
    .await
    .context("upsert accounts")?;
    Ok(())
}

// ---------------- patches ----------------

async fn apply_patch(tx: &mut Transaction<'_, Postgres>, p: &Patch) -> Result<()> {
    match p {
        Patch::Order(o) => {
            sqlx::query(
                "INSERT INTO orders (id, owner, pair, side, order_type, price, quantity, quote_budget, client_id, resting, filled, filled_quote, released, status, created_height, updated_height, tx_id)
                 VALUES ($1, COALESCE($2, ''), COALESCE($3, ''), $4, $5, $6::text::numeric, $7::text::numeric, $8::text::numeric, $9, $10::text::numeric, $11::text::numeric, $12::text::numeric, $13::text::numeric, COALESCE($14, 'open'), COALESCE($15, $16), $16, $17)
                 ON CONFLICT (id) DO UPDATE SET
                   owner = CASE WHEN EXCLUDED.owner <> '' THEN EXCLUDED.owner ELSE orders.owner END,
                   pair = CASE WHEN EXCLUDED.pair <> '' THEN EXCLUDED.pair ELSE orders.pair END,
                   side = COALESCE(EXCLUDED.side, orders.side), order_type = COALESCE(EXCLUDED.order_type, orders.order_type),
                   price = COALESCE(EXCLUDED.price, orders.price), quantity = COALESCE(EXCLUDED.quantity, orders.quantity), quote_budget = COALESCE(EXCLUDED.quote_budget, orders.quote_budget),
                   client_id = COALESCE(EXCLUDED.client_id, orders.client_id), resting = COALESCE(EXCLUDED.resting, orders.resting),
                   filled = orders.filled + EXCLUDED.filled, filled_quote = orders.filled_quote + EXCLUDED.filled_quote, released = COALESCE(EXCLUDED.released, orders.released),
                   status = CASE WHEN $14 IS NOT NULL THEN $14 WHEN orders.status IN ('cancelled', 'filled') THEN orders.status
                                 WHEN orders.quantity IS NOT NULL AND orders.filled + EXCLUDED.filled >= orders.quantity THEN 'filled'
                                 WHEN EXCLUDED.filled > 0 THEN 'partially_filled' ELSE orders.status END,
                   created_height = LEAST(orders.created_height, EXCLUDED.created_height), updated_height = GREATEST(orders.updated_height, EXCLUDED.updated_height),
                   tx_id = COALESCE(orders.tx_id, EXCLUDED.tx_id)",
            )
            .bind(o.id)
            .bind(&o.owner)
            .bind(&o.pair)
            .bind(&o.side)
            .bind(&o.order_type)
            .bind(os(o.price))
            .bind(os(o.quantity))
            .bind(os(o.quote_budget))
            .bind(o.client_id)
            .bind(os(o.resting))
            .bind(s(o.filled_delta))
            .bind(s(o.filled_quote_delta))
            .bind(os(o.released))
            .bind(&o.status)
            .bind(o.created_height)
            .bind(o.updated_height)
            .bind(&o.tx_id)
            .execute(&mut **tx)
            .await
            .context("order patch")?;
        }
        Patch::Offer(o) => apply_offer(tx, o).await?,
        Patch::Trade(t) => {
            let history = t
                .history
                .clone()
                .map(|h| Value::Array(vec![h]))
                .unwrap_or(json!([]));
            sqlx::query(
                "INSERT INTO trades (id, offer_id, buyer, seller, asset, amount, fee, fiat_amount, fiat_currency, status, started_height, started_at, deadline, paid_at, closed_height, updated_height, tx_id, dispute, history)
                 VALUES ($1, $2, $3, $4, $5, $6::text::numeric, $7::text::numeric, $8::text::numeric, $9, COALESCE($10, 'funded'), $11, $12, $13, $14, $15, $16, $17, $18, $19)
                 ON CONFLICT (id) DO UPDATE SET offer_id = COALESCE(EXCLUDED.offer_id, trades.offer_id), buyer = COALESCE(EXCLUDED.buyer, trades.buyer), seller = COALESCE(EXCLUDED.seller, trades.seller),
                   asset = COALESCE(EXCLUDED.asset, trades.asset), amount = COALESCE(EXCLUDED.amount, trades.amount), fee = COALESCE(EXCLUDED.fee, trades.fee),
                   fiat_amount = COALESCE(EXCLUDED.fiat_amount, trades.fiat_amount), fiat_currency = COALESCE(EXCLUDED.fiat_currency, trades.fiat_currency),
                   status = COALESCE($10, trades.status), started_height = COALESCE(trades.started_height, EXCLUDED.started_height), started_at = COALESCE(trades.started_at, EXCLUDED.started_at),
                   deadline = COALESCE(EXCLUDED.deadline, trades.deadline), paid_at = COALESCE(EXCLUDED.paid_at, trades.paid_at), closed_height = COALESCE(EXCLUDED.closed_height, trades.closed_height),
                   updated_height = GREATEST(trades.updated_height, EXCLUDED.updated_height), tx_id = COALESCE(trades.tx_id, EXCLUDED.tx_id),
                   dispute = CASE WHEN EXCLUDED.dispute IS NULL THEN trades.dispute ELSE COALESCE(trades.dispute, '{}'::jsonb) || EXCLUDED.dispute END,
                   history = trades.history || EXCLUDED.history",
            )
            .bind(t.id)
            .bind(t.offer_id)
            .bind(&t.buyer)
            .bind(&t.seller)
            .bind(&t.asset)
            .bind(os(t.amount))
            .bind(os(t.fee))
            .bind(os(t.fiat_amount))
            .bind(&t.fiat_currency)
            .bind(&t.status)
            .bind(t.started_height)
            .bind(t.started_at)
            .bind(t.deadline)
            .bind(t.paid_at)
            .bind(t.closed_height)
            .bind(t.updated_height)
            .bind(&t.tx_id)
            .bind(&t.dispute)
            .bind(history)
            .execute(&mut **tx)
            .await
            .context("trade patch")?;
        }
        Patch::Deposit(d) => {
            sqlx::query(
                "INSERT INTO deposits (key, chain, asset, owner, amount, status, tx_hash, external_index, deposit_index, external_height, votes, height, tx_id, release_height, updated_height)
                 VALUES ($1, $2, $3, $4, $5::text::numeric, $6, $7, $8, $9, $10, $11, $12, $13, $14, $12)
                 ON CONFLICT (key) DO UPDATE SET chain = COALESCE(EXCLUDED.chain, deposits.chain), asset = COALESCE(EXCLUDED.asset, deposits.asset), owner = COALESCE(EXCLUDED.owner, deposits.owner),
                   amount = COALESCE(EXCLUDED.amount, deposits.amount), status = EXCLUDED.status, tx_hash = COALESCE(EXCLUDED.tx_hash, deposits.tx_hash),
                   external_index = COALESCE(EXCLUDED.external_index, deposits.external_index), deposit_index = COALESCE(EXCLUDED.deposit_index, deposits.deposit_index),
                   external_height = COALESCE(EXCLUDED.external_height, deposits.external_height), votes = COALESCE(EXCLUDED.votes, deposits.votes),
                   tx_id = COALESCE(EXCLUDED.tx_id, deposits.tx_id), release_height = COALESCE(EXCLUDED.release_height, deposits.release_height),
                   updated_height = GREATEST(deposits.updated_height, EXCLUDED.updated_height)",
            )
            .bind(&d.key)
            .bind(&d.chain)
            .bind(&d.asset)
            .bind(&d.owner)
            .bind(os(d.amount))
            .bind(&d.status)
            .bind(&d.tx_hash)
            .bind(d.external_index)
            .bind(d.deposit_index)
            .bind(d.external_height)
            .bind(d.votes)
            .bind(d.height)
            .bind(&d.tx_id)
            .bind(d.release_height)
            .execute(&mut **tx)
            .await
            .context("deposit patch")?;
        }
        Patch::Outbound(o) => {
            sqlx::query(
                "INSERT INTO outbounds (id, owner, asset, chain, to_addr, amount, status, batch_id, tx_hash, refunded, created_height, confirmed_height, updated_height, tx_id)
                 VALUES ($1, $2, $3, $4, $5, $6::text::numeric, COALESCE($7, 'queued'), $8, $9, $10::text::numeric, $11, $12, $13, $14)
                 ON CONFLICT (id) DO UPDATE SET owner = COALESCE(EXCLUDED.owner, outbounds.owner), asset = COALESCE(EXCLUDED.asset, outbounds.asset), chain = COALESCE(EXCLUDED.chain, outbounds.chain),
                   to_addr = COALESCE(EXCLUDED.to_addr, outbounds.to_addr), amount = COALESCE(EXCLUDED.amount, outbounds.amount), status = COALESCE($7, outbounds.status),
                   batch_id = COALESCE(EXCLUDED.batch_id, outbounds.batch_id), tx_hash = COALESCE(EXCLUDED.tx_hash, outbounds.tx_hash), refunded = COALESCE(EXCLUDED.refunded, outbounds.refunded),
                   created_height = COALESCE(outbounds.created_height, EXCLUDED.created_height), confirmed_height = COALESCE(EXCLUDED.confirmed_height, outbounds.confirmed_height),
                   updated_height = GREATEST(outbounds.updated_height, EXCLUDED.updated_height), tx_id = COALESCE(outbounds.tx_id, EXCLUDED.tx_id)",
            )
            .bind(o.id)
            .bind(&o.owner)
            .bind(&o.asset)
            .bind(&o.chain)
            .bind(&o.to)
            .bind(os(o.amount))
            .bind(&o.status)
            .bind(o.batch_id)
            .bind(&o.tx_hash)
            .bind(os(o.refunded))
            .bind(o.created_height)
            .bind(o.confirmed_height)
            .bind(o.updated_height)
            .bind(&o.tx_id)
            .execute(&mut **tx)
            .await
            .context("outbound patch")?;
        }
        Patch::Proposal(p) => {
            let (choice, weight) = p
                .tally
                .clone()
                .map(|(c, w)| (Some(c), s(w)))
                .unwrap_or((None, "0".into()));
            sqlx::query(
                "INSERT INTO proposals (id, proposer, title, description, kind, status, submit_height, executed_ok, updated_height, tx_id, yes, no, abstain, veto)
                 VALUES ($1, $2, $3, $4, $5, COALESCE($6, 'voting'), $7, $8, $9, $10,
                   CASE WHEN $11 = 'yes' THEN $12::text::numeric ELSE 0 END, CASE WHEN $11 = 'no' THEN $12::text::numeric ELSE 0 END,
                   CASE WHEN $11 = 'abstain' THEN $12::text::numeric ELSE 0 END, CASE WHEN $11 = 'veto' THEN $12::text::numeric ELSE 0 END)
                 ON CONFLICT (id) DO UPDATE SET proposer = COALESCE(EXCLUDED.proposer, proposals.proposer), title = COALESCE(EXCLUDED.title, proposals.title),
                   description = COALESCE(EXCLUDED.description, proposals.description), kind = COALESCE(EXCLUDED.kind, proposals.kind), status = COALESCE($6, proposals.status),
                   submit_height = COALESCE(proposals.submit_height, EXCLUDED.submit_height), executed_ok = COALESCE(EXCLUDED.executed_ok, proposals.executed_ok),
                   updated_height = GREATEST(proposals.updated_height, EXCLUDED.updated_height), tx_id = COALESCE(proposals.tx_id, EXCLUDED.tx_id),
                   yes = proposals.yes + EXCLUDED.yes, no = proposals.no + EXCLUDED.no, abstain = proposals.abstain + EXCLUDED.abstain, veto = proposals.veto + EXCLUDED.veto",
            )
            .bind(p.id)
            .bind(&p.proposer)
            .bind(&p.title)
            .bind(&p.description)
            .bind(&p.kind)
            .bind(&p.status)
            .bind(p.submit_height)
            .bind(p.executed_ok)
            .bind(p.updated_height)
            .bind(&p.tx_id)
            .bind(choice)
            .bind(weight)
            .execute(&mut **tx)
            .await
            .context("proposal patch")?;
        }
        Patch::Vote(v) => {
            sqlx::query(
                "INSERT INTO votes (proposal_id, voter, choice, weight, height, tx_id) VALUES ($1, $2, $3, $4::text::numeric, $5, $6)
                 ON CONFLICT (proposal_id, voter) DO UPDATE SET choice = COALESCE(EXCLUDED.choice, votes.choice), weight = EXCLUDED.weight, height = EXCLUDED.height, tx_id = EXCLUDED.tx_id",
            )
            .bind(v.proposal_id)
            .bind(&v.voter)
            .bind(&v.choice)
            .bind(s(v.weight))
            .bind(v.height)
            .bind(&v.tx_id)
            .execute(&mut **tx)
            .await
            .context("vote")?;
        }
        Patch::Param(p) => {
            sqlx::query(
                "INSERT INTO param_history (height, tx_index, event_index, timestamp, key, from_value, to_value, tx_id) VALUES ($1, $2, $3, $4, $5, $6::text::numeric, $7::text::numeric, $8)
                 ON CONFLICT DO NOTHING",
            )
            .bind(p.height)
            .bind(p.tx_index)
            .bind(p.event_index)
            .bind(p.timestamp)
            .bind(&p.key)
            .bind(os(p.from))
            .bind(s(p.to))
            .bind(&p.tx_id)
            .execute(&mut **tx)
            .await
            .context("param history")?;
        }
        Patch::Epoch(e) => {
            sqlx::query("INSERT INTO epochs (epoch, start_height, validator_count) VALUES ($1, $2, $3) ON CONFLICT (epoch) DO UPDATE SET start_height = EXCLUDED.start_height, validator_count = EXCLUDED.validator_count")
                .bind(e.epoch)
                .bind(e.start_height)
                .bind(e.validator_count)
                .execute(&mut **tx)
                .await
                .context("epoch")?;
        }
    }
    Ok(())
}

async fn apply_offer(
    tx: &mut Transaction<'_, Postgres>,
    o: &crate::materialize::OfferPatch,
) -> Result<()> {
    let spec = o.spec.clone().unwrap_or(Value::Null);
    sqlx::query(
        "INSERT INTO offers (id, owner, side, asset, fiat_currency, payment_method, margin_bps, fixed_price, min_amount, max_amount, payment_window_secs, country, min_tier, status, created_height, updated_height, tx_id)
         VALUES ($1, COALESCE($2, ''), $3, $4, $5, $6, $7, $8::text::numeric, $9::text::numeric, $10::text::numeric, $11, $12, $13, COALESCE($14, 'open'), COALESCE($15, $16), $16, $17)
         ON CONFLICT (id) DO UPDATE SET owner = CASE WHEN EXCLUDED.owner <> '' THEN EXCLUDED.owner ELSE offers.owner END,
           side = COALESCE(EXCLUDED.side, offers.side), asset = COALESCE(EXCLUDED.asset, offers.asset), fiat_currency = COALESCE(EXCLUDED.fiat_currency, offers.fiat_currency),
           payment_method = COALESCE(EXCLUDED.payment_method, offers.payment_method), margin_bps = COALESCE(EXCLUDED.margin_bps, offers.margin_bps), fixed_price = COALESCE(EXCLUDED.fixed_price, offers.fixed_price),
           min_amount = COALESCE(EXCLUDED.min_amount, offers.min_amount), max_amount = COALESCE(EXCLUDED.max_amount, offers.max_amount),
           payment_window_secs = COALESCE(EXCLUDED.payment_window_secs, offers.payment_window_secs), country = COALESCE(EXCLUDED.country, offers.country), min_tier = COALESCE(EXCLUDED.min_tier, offers.min_tier),
           status = COALESCE($14, offers.status), created_height = LEAST(offers.created_height, EXCLUDED.created_height), updated_height = GREATEST(offers.updated_height, EXCLUDED.updated_height),
           tx_id = COALESCE(offers.tx_id, EXCLUDED.tx_id)",
    )
    .bind(o.id)
    .bind(&o.owner)
    .bind(spec.get("side").and_then(Value::as_str).map(|x| x.to_ascii_lowercase()))
    .bind(field_str(&spec, "asset"))
    .bind(field_str(&spec, "fiat_currency"))
    .bind(field_str(&spec, "payment_method"))
    .bind(spec.get("margin_bps").and_then(Value::as_i64).map(|x| x as i32))
    .bind(os(field_amount(&spec, "fixed_price")))
    .bind(os(field_amount(&spec, "min_amount")))
    .bind(os(field_amount(&spec, "max_amount")))
    .bind(field_u64(&spec, "payment_window_secs").map(|x| x as i32))
    .bind(field_str(&spec, "country"))
    .bind(field_u64(&spec, "min_tier").map(|x| x as i32))
    .bind(&o.status)
    .bind(o.created_height)
    .bind(o.updated_height)
    .bind(&o.tx_id)
    .execute(&mut **tx)
    .await
    .context("offer patch")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_statuses() {
        assert_eq!(snake(Some(&json!("PartiallyFilled"))), "partially_filled");
        assert_eq!(snake(Some(&json!("Open"))), "open");
        assert_eq!(snake(Some(&json!({"Split": {"buyer_bps": 1}}))), "split");
        assert_eq!(snake(None), "");
    }
}
