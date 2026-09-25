//! `keel load`: throughput and latency against a running devnet.
//!
//! 1. Fund `--accounts` fresh keys (seeds 10_000..) with KUSD and BTC.BTC
//!    from the devnet seed accounts (0..3), waiting for receipts.
//! 2. Pre-sign `--per-account` limit orders per key (alternating sides at
//!    prices that cross), then submit them from `--threads` workers,
//!    round-robin over `--rpc` and `--extra-rpc` nodes.
//! 3. Poll `/v1/status` until the applied height stops advancing the
//!    mempool, and report accepted actions per second and the p50/p99
//!    submit-to-receipt latency of a sample.

// A load tool measures wall time and prints rates; it is not chain code.
#![allow(
    clippy::float_arithmetic,
    clippy::disallowed_methods,
    clippy::cast_precision_loss
)]

use anyhow::{anyhow, Context as _};
use keel_actions::{Action, PlaceOrder, SignedAction, Transfer};
use keel_crypto::Keypair;
use keel_types::{Asset, OrderType, Side};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(clap::Args, Debug)]
pub struct Args {
    #[arg(long, default_value_t = 100)]
    pub accounts: u64,
    #[arg(long, default_value_t = 200)]
    pub per_account: u64,
    #[arg(long, default_value_t = 16)]
    pub threads: usize,
    /// Additional RPC base URLs to spread submissions over.
    #[arg(long, value_delimiter = ',')]
    pub extra_rpc: Vec<String>,
    /// Devnet seed accounts that hold genesis funds.
    #[arg(long, value_delimiter = ',', default_value = "0,1,2,3")]
    pub funders: Vec<u64>,
    /// Sample size for receipt latency.
    #[arg(long, default_value_t = 200)]
    pub latency_samples: usize,
}

struct Http {
    base: String,
    agent: ureq::Agent,
}

impl Http {
    fn new(base: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent,
        }
    }
    fn get(&self, path: &str) -> anyhow::Result<Value> {
        let mut r = self.agent.get(&format!("{}{}", self.base, path)).call()?;
        Ok(r.body_mut().read_json::<Value>()?)
    }
    fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let mut r = self
            .agent
            .post(&format!("{}{}", self.base, path))
            .send_json(body)?;
        Ok(r.body_mut().read_json::<Value>()?)
    }
    fn nonce(&self, k: &Keypair) -> anyhow::Result<u64> {
        let v = self.get(&format!("/v1/accounts/{}", k.address().to_hex()))?;
        Ok(v["nonce"].as_u64().unwrap_or(0))
    }
    fn height(&self) -> anyhow::Result<u64> {
        Ok(self.get("/v1/status")?["height"].as_u64().unwrap_or(0))
    }
    fn submit(&self, sa: &SignedAction) -> anyhow::Result<(bool, String)> {
        let v = self.post("/v1/actions", &serde_json::to_value(sa)?)?;
        let ok = v["admitted"].as_bool().unwrap_or(false);
        let id = v["tx_id"].as_str().unwrap_or_default().to_string();
        if !ok {
            return Ok((false, v["error"].to_string()));
        }
        Ok((true, id))
    }
    fn receipt_ok(&self, tx_id: &str) -> anyhow::Result<Option<bool>> {
        let v = self.get(&format!("/v1/receipts/{tx_id}"))?;
        Ok(v.get("ok").and_then(Value::as_bool))
    }
}

fn wait_receipt(http: &Http, tx_id: &str, timeout: Duration) -> anyhow::Result<bool> {
    let start = Instant::now();
    loop {
        if let Some(ok) = http.receipt_ok(tx_id)? {
            return Ok(ok);
        }
        if start.elapsed() > timeout {
            return Err(anyhow!("receipt {tx_id} not found within {timeout:?}"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn run(args: &Args, rpc: &str, chain_id: u32) -> anyhow::Result<()> {
    let mut rpcs: Vec<Arc<Http>> = vec![Arc::new(Http::new(rpc))];
    rpcs.extend(args.extra_rpc.iter().map(|u| Arc::new(Http::new(u))));
    let main = rpcs[0].clone();
    let usds = Asset::new("KUSD");
    let btc = Asset::new("BTC.BTC");

    // 1. Fund.
    let keys: Vec<Keypair> = (0..args.accounts)
        .map(|i| Keypair::from_seed(10_000 + i))
        .collect();
    let funders: Vec<Keypair> = args
        .funders
        .iter()
        .map(|s| Keypair::from_seed(*s))
        .collect();
    eprintln!(
        "[load] funding {} accounts from {} devnet seeds",
        keys.len(),
        funders.len()
    );
    let t = Instant::now();
    let mut last_ids = Vec::new();
    // Devnet genesis funds each seed with 1,000,000 KUSD and 100 BTC; split
    // 80% of that evenly across this funder's share of the accounts.
    let share = keys.len().div_ceil(funders.len()).max(1) as u128;
    let usds_each = 800_000_000_000u128 / share;
    let btc_each = 8_000_000_000u128 / share;
    for (fi, f) in funders.iter().enumerate() {
        let mut nonce = main.nonce(f)?;
        for (ki, k) in keys.iter().enumerate() {
            if ki % funders.len() != fi {
                continue;
            }
            for (asset, amount) in [(&usds, usds_each), (&btc, btc_each)] {
                let sa = SignedAction::sign(
                    f,
                    nonce,
                    chain_id,
                    Action::Transfer(Transfer {
                        to: k.address(),
                        asset: asset.clone(),
                        amount,
                        memo: None,
                    }),
                );
                nonce += 1;
                let (ok, id) = main.submit(&sa)?;
                if !ok {
                    return Err(anyhow!("funding refused: {id}"));
                }
                last_ids.push(id);
            }
        }
    }
    for id in last_ids.iter().rev().take(funders.len()) {
        if !wait_receipt(&main, id, Duration::from_secs(60))? {
            return Err(anyhow!("funding transfer {id} failed"));
        }
    }
    eprintln!("[load] funded in {:.1}s", t.elapsed().as_secs_f64());

    // 2. Pre-sign.
    let t = Instant::now();
    let mut jobs: Vec<Vec<SignedAction>> = Vec::with_capacity(keys.len());
    for (i, k) in keys.iter().enumerate() {
        let mut list = Vec::with_capacity(args.per_account as usize);
        for round in 0..args.per_account {
            let sell = (i + round as usize).is_multiple_of(2);
            let price = 60_000_000_000u128 + (round % 20) as u128 * 10_000;
            list.push(SignedAction::sign(
                k,
                round,
                chain_id,
                Action::PlaceOrder(PlaceOrder {
                    pair: "BTC-KUSD".into(),
                    side: if sell { Side::Sell } else { Side::Buy },
                    order_type: OrderType::Limit,
                    price: Some(price),
                    quantity: Some(10_000),
                    quote_budget: None,
                    client_id: Some(round),
                }),
            ));
        }
        jobs.push(list);
    }
    let total = keys.len() as u64 * args.per_account;
    eprintln!(
        "[load] pre-signed {total} orders in {:.1}s",
        t.elapsed().as_secs_f64()
    );

    // 3. Submit concurrently; each worker owns whole accounts so nonces
    //    arrive in order on one node.
    let accepted = Arc::new(AtomicU64::new(0));
    let refused = Arc::new(AtomicU64::new(0));
    type Sample = (Instant, String, Arc<Http>);
    let samples: Arc<std::sync::Mutex<Vec<Sample>>> = Arc::new(Default::default());
    let start_height = main.height()?;
    let t_submit = Instant::now();
    let threads = args.threads.max(1);
    let jobs = Arc::new(jobs);
    let handles: Vec<_> = (0..threads)
        .map(|w| {
            let jobs = jobs.clone();
            let rpcs = rpcs.clone();
            let accepted = accepted.clone();
            let refused = refused.clone();
            let samples = samples.clone();
            let sample_every = (total as usize / args.latency_samples.max(1)).max(1);
            std::thread::spawn(move || {
                let http = rpcs[w % rpcs.len()].clone();
                let mut n = 0usize;
                for (ai, list) in jobs.iter().enumerate() {
                    if ai % threads != w {
                        continue;
                    }
                    for sa in list {
                        // The mempool pipelines a bounded nonce window per
                        // signer; a "bad nonce" refusal here means "too far
                        // ahead", so wait for blocks and retry.
                        let mut attempts = 0;
                        loop {
                            match http.submit(sa) {
                                Ok((true, id)) => {
                                    accepted.fetch_add(1, Ordering::Relaxed);
                                    if n.is_multiple_of(sample_every) {
                                        samples.lock().expect("samples").push((
                                            Instant::now(),
                                            id,
                                            http.clone(),
                                        ));
                                    }
                                    break;
                                }
                                Ok((false, why)) if why.contains("bad nonce") && attempts < 200 => {
                                    attempts += 1;
                                    std::thread::sleep(Duration::from_millis(100));
                                }
                                Ok((false, why)) => {
                                    if refused.fetch_add(1, Ordering::Relaxed) < 3 {
                                        eprintln!("[load] refused: {why}");
                                    }
                                    break;
                                }
                                Err(_) => {
                                    refused.fetch_add(1, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }
                        n += 1;
                    }
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    let submit_secs = t_submit.elapsed().as_secs_f64();
    let acc = accepted.load(Ordering::Relaxed);
    let refd = refused.load(Ordering::Relaxed);
    eprintln!(
        "[load] submitted: accepted={acc} refused={refd} in {submit_secs:.1}s ({:.0}/s over HTTP)",
        acc as f64 / submit_secs
    );

    // 4. Drain: wait until the mempool is empty on the main node.
    let t_drain = Instant::now();
    loop {
        let st = main.get("/v1/status")?;
        let mempool = st["mempool"].as_u64().unwrap_or(0);
        if mempool == 0 && t_drain.elapsed() > Duration::from_secs(2) {
            break;
        }
        if t_drain.elapsed() > Duration::from_secs(300) {
            return Err(anyhow!("mempool did not drain: {mempool} left"));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let end_height = main.height()?;
    let applied_secs = t_submit.elapsed().as_secs_f64();

    // 5. Latency sample: submit time -> first receipt visible.
    let mut lat_ms: Vec<f64> = Vec::new();
    for (at, id, http) in samples.lock().expect("samples").iter() {
        // Receipts are already applied; approximate latency by the block
        // timestamp of the receipt when available, else skip.
        let v = http
            .get(&format!("/v1/receipts/{id}"))
            .unwrap_or(Value::Null);
        if let Some(ts) = v.get("timestamp").and_then(Value::as_u64) {
            let submitted_ms = unix_ms_at(*at, t_submit);
            if ts >= submitted_ms {
                lat_ms.push((ts - submitted_ms) as f64);
            }
        }
    }
    lat_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| {
        lat_ms
            .get(((lat_ms.len() as f64 * p) as usize).min(lat_ms.len().saturating_sub(1)))
            .copied()
    };

    // 6. Sanity: every node agrees on the tip hash.
    let mut hashes = Vec::new();
    for r in &rpcs {
        let st = r.get("/v1/status")?;
        hashes.push((
            st["height"].as_u64().unwrap_or(0),
            st["state_hash"].as_str().unwrap_or("").to_string(),
        ));
    }
    println!(
        "{}",
        json!({
            "accounts": keys.len(),
            "orders": total,
            "accepted": acc,
            "refused": refd,
            "blocks": end_height.saturating_sub(start_height),
            "submit_rate_per_s": (acc as f64 / submit_secs).round(),
            "applied_rate_per_s": (acc as f64 / applied_secs).round(),
            "latency_ms": { "samples": lat_ms.len(), "p50": pct(0.5), "p90": pct(0.9), "p99": pct(0.99) },
            "tips": hashes,
        })
    );
    Ok(())
}

fn unix_ms_at(at: Instant, _anchor: Instant) -> u64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now_ms.saturating_sub(at.elapsed().as_millis() as u64)
}

#[allow(dead_code)]
fn _ctx() -> anyhow::Result<()> {
    Err(anyhow!("unused")).context("load")
}
