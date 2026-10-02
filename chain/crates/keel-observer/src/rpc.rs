//! The Keel node as seen by the observer: vault registrations, deposit
//! address maps, batched outbounds, nonces and action submission. The
//! exact routes and JSON shapes are listed in `API.md`.

use crate::{events::EventsView, state::StateFile};
use async_trait::async_trait;
use keel_actions::{Action, Chain, SignedAction};
use keel_crypto::Keypair;
use keel_types::{Address, Amount};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub chain_id: u32,
    pub height: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ChainParams {
    pub confirmations_btc: u32,
    pub confirmations_eth: u32,
    pub confirmations_tron: u32,
    pub outbound_batch_interval_blocks: u64,
}

impl ChainParams {
    pub fn confirmations(&self, chain: Chain) -> u32 {
        match chain {
            Chain::Bitcoin => self.confirmations_btc,
            Chain::Ethereum => self.confirmations_eth,
            Chain::Tron => self.confirmations_tron,
        }
    }
}

/// The active vault of one chain plus its deposit-index map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultView {
    pub chain: Chain,
    pub epoch: u64,
    pub public_key: [u8; 33],
    pub chain_code: [u8; 32],
    pub signers: Vec<Address>,
    pub threshold: u32,
    pub next_deposit_index: u64,
    /// deposit index → owner.
    pub owners: BTreeMap<u64, Address>,
    /// Set for a client-owned custody vault: whose it is and where its
    /// signer answers (`/v1/custody`).
    pub custodian: Option<Address>,
    pub signer_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundRow {
    pub id: u64,
    pub owner: Address,
    pub asset: String,
    pub chain: Chain,
    pub to: String,
    pub amount: Amount,
    pub fee_asset: String,
    pub fee_estimate: Amount,
    pub status: String,
    pub batch_id: Option<u64>,
    pub created_height: u64,
    #[serde(default)]
    pub tx_hash: Option<String>,
    /// The custody vault this outbound is drawn from (`None` = network).
    #[serde(default)]
    pub custodian: Option<Address>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitResult {
    pub admitted: bool,
    pub tx_id: Option<String>,
    pub error: Option<String>,
}

#[async_trait]
pub trait SttRpc: Send + Sync {
    async fn status(&self) -> anyhow::Result<Status>;
    async fn params(&self) -> anyhow::Result<ChainParams>;
    async fn vault(&self, chain: Chain) -> anyhow::Result<Option<VaultView>>;
    /// Every client-owned custody vault (empty when the node has none or
    /// does not serve the route).
    async fn custody_vaults(&self) -> anyhow::Result<Vec<VaultView>> {
        Ok(Vec::new())
    }
    async fn outbounds(&self, status: &str) -> anyhow::Result<Vec<OutboundRow>>;
    async fn nonce(&self, addr: &Address) -> anyhow::Result<u64>;
    async fn submit(&self, action: &SignedAction) -> anyhow::Result<SubmitResult>;
    /// `GET /v1/lightning` (pools, assignments); `Null` when the node predates it.
    async fn lightning(&self) -> anyhow::Result<Value> {
        Ok(Value::Null)
    }
}

// ---------------------------------------------------------------- parsing

fn u64_of(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::String(s) => hex::decode(s.strip_prefix("0x").unwrap_or(s)).ok(),
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_u64().and_then(|b| u8::try_from(b).ok()))
            .collect(),
        _ => None,
    }
}

/// `GET /v1/vaults/{chain}` → the active vault and the deposit map.
pub fn parse_vault(chain: Chain, v: &Value) -> anyhow::Result<Option<VaultView>> {
    let Some(vault) = v.get("vault").filter(|x| !x.is_null()) else {
        return Ok(None);
    };
    let public_key =
        bytes_of(&vault["public_key"]).ok_or_else(|| anyhow::anyhow!("vault.public_key"))?;
    let public_key: [u8; 33] = public_key
        .try_into()
        .map_err(|_| anyhow::anyhow!("vault key must be 33 bytes"))?;
    let chain_code = bytes_of(&vault["chain_code"])
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or_else(|| anyhow::anyhow!("vault.chain_code missing (not an HD vault)"))?;
    let signers = vault["signers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().and_then(Address::from_hex))
                .collect()
        })
        .unwrap_or_default();
    let mut owners = BTreeMap::new();
    if let Some(map) = v.get("deposit_owners").and_then(Value::as_object) {
        for (k, val) in map {
            if let (Ok(i), Some(a)) = (k.parse::<u64>(), val.as_str().and_then(Address::from_hex)) {
                owners.insert(i, a);
            }
        }
    }
    Ok(Some(VaultView {
        chain,
        epoch: u64_of(&vault["epoch"]).unwrap_or(0),
        public_key,
        chain_code,
        signers,
        threshold: u64_of(&vault["threshold"]).unwrap_or(0) as u32,
        next_deposit_index: u64_of(&v["next_deposit_index"]).unwrap_or(1),
        owners,
        custodian: v
            .get("custodian")
            .and_then(Value::as_str)
            .and_then(Address::from_hex),
        signer_url: v
            .get("signer_url")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

/// `GET /v1/custody` → every client-owned vault as a [`VaultView`].
pub fn parse_custody(v: &Value) -> anyhow::Result<Vec<VaultView>> {
    let mut out = Vec::new();
    for entry in v
        .get("vaults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(chain) = entry
            .get("chain")
            .and_then(Value::as_str)
            .and_then(chain_from_json)
        else {
            continue;
        };
        match parse_vault(chain, &entry) {
            Ok(Some(view)) if view.custodian.is_some() => out.push(view),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "custody vault entry skipped"),
        }
    }
    Ok(out)
}

/// `GET /v1/vaults/outbounds` → rows. Tolerates `amount` as number or string.
pub fn parse_outbounds(v: &Value) -> anyhow::Result<Vec<OutboundRow>> {
    let list = v
        .get("outbounds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(list.len());
    for o in list {
        let amount = amount_of(&o["amount"]).ok_or_else(|| anyhow::anyhow!("outbound.amount"))?;
        let fee_estimate = amount_of(&o["fee_estimate"]).unwrap_or(0);
        let chain = match &o["chain"] {
            Value::String(s) => {
                chain_from_json(s).ok_or_else(|| anyhow::anyhow!("outbound.chain {s}"))?
            }
            _ => anyhow::bail!("outbound.chain"),
        };
        out.push(OutboundRow {
            id: u64_of(&o["id"]).ok_or_else(|| anyhow::anyhow!("outbound.id"))?,
            owner: o["owner"]
                .as_str()
                .and_then(Address::from_hex)
                .ok_or_else(|| anyhow::anyhow!("outbound.owner"))?,
            asset: o["asset"].as_str().unwrap_or_default().to_string(),
            chain,
            to: o["to"].as_str().unwrap_or_default().to_string(),
            amount,
            fee_asset: o["fee_asset"].as_str().unwrap_or_default().to_string(),
            fee_estimate,
            status: o["status"].as_str().unwrap_or_default().to_string(),
            batch_id: o.get("batch_id").and_then(u64_of),
            created_height: u64_of(&o["created_height"]).unwrap_or(0),
            tx_hash: o.get("tx_hash").and_then(|t| match t {
                Value::String(s) => Some(s.clone()),
                Value::Array(_) => bytes_of(t).map(hex::encode),
                _ => None,
            }),
            custodian: o
                .get("custodian")
                .and_then(Value::as_str)
                .and_then(Address::from_hex),
        });
    }
    Ok(out)
}

pub fn amount_of(v: &Value) -> Option<Amount> {
    match v {
        Value::Number(n) => n.as_u128(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Chain names as `serde_json` writes the `Chain` enum (`"Bitcoin"`) or
/// as `Chain::as_str` (`"BTC"`).
pub fn chain_from_json(s: &str) -> Option<Chain> {
    Chain::parse(s).or(match s {
        "Bitcoin" => Some(Chain::Bitcoin),
        "Ethereum" => Some(Chain::Ethereum),
        "Tron" => Some(Chain::Tron),
        _ => None,
    })
}

pub fn parse_status(v: &Value) -> anyhow::Result<Status> {
    Ok(Status {
        chain_id: u64_of(&v["chain_id"]).ok_or_else(|| anyhow::anyhow!("status.chain_id"))? as u32,
        height: u64_of(&v["height"]).unwrap_or(0),
    })
}

pub fn parse_params(v: &Value) -> ChainParams {
    let g = |k: &str, d: u64| u64_of(&v[k]).unwrap_or(d);
    ChainParams {
        confirmations_btc: g("confirmations_btc", 2) as u32,
        confirmations_eth: g("confirmations_eth", 12) as u32,
        confirmations_tron: g("confirmations_tron", 19) as u32,
        outbound_batch_interval_blocks: g("outbound_batch_interval_blocks", 20),
    }
}

pub fn parse_submit(status: u16, v: &Value) -> SubmitResult {
    SubmitResult {
        admitted: v["admitted"].as_bool().unwrap_or(status < 300),
        tx_id: v["tx_id"].as_str().map(str::to_string),
        error: v["error"].as_str().map(str::to_string),
    }
}

// ---------------------------------------------------------------- http

/// What the receipts fallback cannot learn from events: the vault key
/// (registered from the config or the `local:` signer) and the signer
/// set (`config::VaultFallback`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FallbackKeys {
    pub public_key: [u8; 33],
    pub chain_code: [u8; 32],
    pub signers: Vec<Address>,
    pub threshold: u32,
}

/// Blocks of receipts the fallback scans on its first refresh (the node
/// keeps the last 10,000).
const RECEIPTS_WINDOW: u64 = 9_000;

pub struct HttpSttRpc {
    base: String,
    client: reqwest::Client,
    fallback: Option<FallbackKeys>,
    view: Mutex<EventsView>,
    /// Serializes refreshes so two loops never fold the same block twice.
    refresh: tokio::sync::Mutex<()>,
    /// Whether `/v1/vaults/{chain}` last answered with a usable vault.
    primary_ok: AtomicBool,
}

impl HttpSttRpc {
    pub fn new(base: &str) -> Self {
        Self::with_fallback(base, None)
    }

    /// With `fallback`, `vault()` and `outbounds()` are answered from the
    /// block receipts (`crate::events`) whenever the node's own view is
    /// empty.
    pub fn with_fallback(base: &str, fallback: Option<FallbackKeys>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            fallback,
            view: Mutex::new(EventsView::default()),
            refresh: tokio::sync::Mutex::new(()),
            primary_ok: AtomicBool::new(false),
        }
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(format!("{}{path}", self.base))
            .send()
            .await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            anyhow::bail!("GET {path}: {status} {v}");
        }
        Ok(v)
    }

    /// Fold every block above the cursor; returns (folded tip, batch interval).
    async fn refresh_view(&self) -> anyhow::Result<(u64, u64)> {
        let _guard = self.refresh.lock().await;
        let height = self.status().await?.height;
        let interval = self.params().await?.outbound_batch_interval_blocks;
        let cursor = self.view.lock().expect("view lock").cursor;
        let from = if cursor == 0 {
            let start = height.saturating_sub(RECEIPTS_WINDOW).max(1);
            if start > 1 {
                tracing::warn!(start, "receipts fallback starts late: deposit indexes assigned before this block are unknown");
            }
            start
        } else {
            cursor + 1
        };
        for h in from..=height {
            let receipts = match self.get(&format!("/v1/blocks/{h}/receipts")).await {
                Ok(v) => v["receipts"].as_array().cloned().unwrap_or_default(),
                Err(e) => {
                    tracing::debug!(height = h, error = %e, "no receipts for block");
                    Vec::new()
                }
            };
            self.view
                .lock()
                .expect("view lock")
                .fold_block(h, &receipts);
        }
        let tip = self.view.lock().expect("view lock").cursor.max(height);
        Ok((tip, interval))
    }

    fn use_fallback(&self) -> bool {
        self.fallback.is_some() && !self.primary_ok.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl SttRpc for HttpSttRpc {
    async fn status(&self) -> anyhow::Result<Status> {
        parse_status(&self.get("/v1/status").await?)
    }

    async fn params(&self) -> anyhow::Result<ChainParams> {
        Ok(parse_params(&self.get("/v1/params").await?))
    }

    async fn custody_vaults(&self) -> anyhow::Result<Vec<VaultView>> {
        match self.get("/v1/custody").await {
            Ok(v) => parse_custody(&v),
            Err(e) => {
                tracing::debug!(error = %e, "custody route unavailable");
                Ok(Vec::new())
            }
        }
    }

    async fn vault(&self, chain: Chain) -> anyhow::Result<Option<VaultView>> {
        let v = self.get(&format!("/v1/vaults/{}", chain.as_str())).await?;
        match parse_vault(chain, &v) {
            Ok(Some(view)) => {
                self.primary_ok.store(true, Ordering::Relaxed);
                return Ok(Some(view));
            }
            Ok(None) => {}
            Err(e) if self.fallback.is_none() => return Err(e),
            Err(e) => {
                tracing::debug!(error = %e, "vault route unusable, using the receipts fallback")
            }
        }
        let Some(fb) = &self.fallback else {
            return Ok(None);
        };
        self.primary_ok.store(false, Ordering::Relaxed);
        self.refresh_view().await?;
        let view = self.view.lock().expect("view lock");
        let Some(epoch) = view.epochs.get(&chain).copied() else {
            return Ok(None);
        };
        Ok(Some(VaultView {
            chain,
            epoch,
            public_key: fb.public_key,
            chain_code: fb.chain_code,
            signers: fb.signers.clone(),
            threshold: fb.threshold,
            next_deposit_index: view.next_deposit_index(chain),
            owners: view.owners.get(&chain).cloned().unwrap_or_default(),
            custodian: None,
            signer_url: None,
        }))
    }

    async fn outbounds(&self, status: &str) -> anyhow::Result<Vec<OutboundRow>> {
        if self.use_fallback() {
            let (tip, interval) = self.refresh_view().await?;
            let view = self.view.lock().expect("view lock");
            return Ok(view
                .outbounds(interval, tip)
                .into_iter()
                .filter(|o| o.status == status)
                .collect());
        }
        parse_outbounds(
            &self
                .get(&format!("/v1/vaults/outbounds?status={status}"))
                .await?,
        )
    }

    async fn lightning(&self) -> anyhow::Result<Value> {
        match self.get("/v1/lightning").await {
            Ok(v) => Ok(v),
            Err(e) if e.to_string().contains("404") => Ok(Value::Null),
            Err(e) => Err(e),
        }
    }

    async fn nonce(&self, addr: &Address) -> anyhow::Result<u64> {
        let v = self.get(&format!("/v1/accounts/{}", addr.to_hex())).await?;
        u64_of(&v["nonce"]).ok_or_else(|| anyhow::anyhow!("account.nonce"))
    }

    async fn submit(&self, action: &SignedAction) -> anyhow::Result<SubmitResult> {
        let resp = self
            .client
            .post(format!("{}/v1/actions", self.base))
            .json(action)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        Ok(parse_submit(status, &v))
    }
}

// ---------------------------------------------------------------- submitter

/// Signs and submits actions with a locally tracked nonce, recording
/// idempotency keys in the state file.
pub struct Submitter {
    rpc: Arc<dyn SttRpc>,
    key: Keypair,
    chain_id: u32,
    state: Arc<StateFile>,
    next_nonce: Mutex<Option<u64>>,
}

impl Submitter {
    pub fn new(rpc: Arc<dyn SttRpc>, key: Keypair, chain_id: u32, state: Arc<StateFile>) -> Self {
        Self {
            rpc,
            key,
            chain_id,
            state,
            next_nonce: Mutex::new(None),
        }
    }

    pub fn address(&self) -> Address {
        self.key.address()
    }

    async fn take_nonce(&self) -> anyhow::Result<u64> {
        let cached = *self.next_nonce.lock().expect("nonce lock");
        match cached {
            Some(n) => Ok(n),
            None => {
                let n = self.rpc.nonce(&self.key.address()).await?;
                *self.next_nonce.lock().expect("nonce lock") = Some(n);
                Ok(n)
            }
        }
    }

    /// Submit once per `key`; a `None` key submits unconditionally
    /// (fee reports).
    pub async fn submit(&self, key: Option<&str>, action: Action) -> anyhow::Result<SubmitResult> {
        if let Some(k) = key {
            if self.state.is_submitted(k) {
                return Ok(SubmitResult {
                    admitted: true,
                    tx_id: None,
                    error: Some("already submitted".into()),
                });
            }
        }
        let nonce = self.take_nonce().await?;
        let signed = SignedAction::sign(&self.key, nonce, self.chain_id, action.clone());
        let mut res = self.rpc.submit(&signed).await?;
        if !res.admitted
            && res
                .error
                .as_deref()
                .is_some_and(|e| e.contains("bad nonce"))
        {
            // Another process signs with this key too (the vote timer, an
            // onboarding run): resync from the node and retry once now
            // instead of losing this pass.
            *self.next_nonce.lock().expect("nonce lock") = None;
            let nonce = self.take_nonce().await?;
            let signed = SignedAction::sign(&self.key, nonce, self.chain_id, action);
            res = self.rpc.submit(&signed).await?;
            if res.admitted {
                *self.next_nonce.lock().expect("nonce lock") = Some(nonce + 1);
                if let Some(k) = key {
                    self.state
                        .mark_submitted(k, res.tx_id.as_deref().unwrap_or(""))?;
                }
                return Ok(res);
            }
        } else if res.admitted {
            *self.next_nonce.lock().expect("nonce lock") = Some(nonce + 1);
            if let Some(k) = key {
                self.state
                    .mark_submitted(k, res.tx_id.as_deref().unwrap_or(""))?;
            }
            return Ok(res);
        }
        // Any refusal may be a nonce disagreement: resync next time.
        *self.next_nonce.lock().expect("nonce lock") = None;
        tracing::warn!(key = key.unwrap_or("-"), error = ?res.error, "action refused");
        Ok(res)
    }
}

// ---------------------------------------------------------------- mock

/// In-memory node for tests: fixed vaults/outbounds, records submissions.
#[derive(Default)]
pub struct MockSttRpc {
    pub chain_id: u32,
    pub params: ChainParams,
    pub vaults: Mutex<BTreeMap<Chain, VaultView>>,
    pub outbounds: Mutex<Vec<OutboundRow>>,
    pub nonces: Mutex<BTreeMap<Address, u64>>,
    pub submitted: Mutex<Vec<SignedAction>>,
    /// Refuse every submission with this error.
    pub refuse: Mutex<Option<String>>,
}

#[async_trait]
impl SttRpc for MockSttRpc {
    async fn status(&self) -> anyhow::Result<Status> {
        Ok(Status {
            chain_id: self.chain_id,
            height: 100,
        })
    }

    async fn params(&self) -> anyhow::Result<ChainParams> {
        Ok(self.params.clone())
    }

    async fn vault(&self, chain: Chain) -> anyhow::Result<Option<VaultView>> {
        Ok(self.vaults.lock().expect("lock").get(&chain).cloned())
    }

    async fn outbounds(&self, status: &str) -> anyhow::Result<Vec<OutboundRow>> {
        Ok(self
            .outbounds
            .lock()
            .expect("lock")
            .iter()
            .filter(|o| o.status == status)
            .cloned()
            .collect())
    }

    async fn nonce(&self, addr: &Address) -> anyhow::Result<u64> {
        Ok(self
            .nonces
            .lock()
            .expect("lock")
            .get(addr)
            .copied()
            .unwrap_or(0))
    }

    async fn submit(&self, action: &SignedAction) -> anyhow::Result<SubmitResult> {
        if let Some(e) = self.refuse.lock().expect("lock").clone() {
            return Ok(SubmitResult {
                admitted: false,
                tx_id: None,
                error: Some(e),
            });
        }
        if !action.verify() {
            return Ok(SubmitResult {
                admitted: false,
                tx_id: None,
                error: Some("bad signature".into()),
            });
        }
        let mut nonces = self.nonces.lock().expect("lock");
        let expected = nonces.get(&action.signer()).copied().unwrap_or(0);
        if action.envelope.nonce != expected {
            return Ok(SubmitResult {
                admitted: false,
                tx_id: None,
                error: Some(format!("nonce {} != {expected}", action.envelope.nonce)),
            });
        }
        nonces.insert(action.signer(), expected + 1);
        self.submitted.lock().expect("lock").push(action.clone());
        Ok(SubmitResult {
            admitted: true,
            tx_id: Some(hex::encode(action.id())),
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_actions::CHAIN_ID_DEVNET;
    use serde_json::json;

    #[test]
    fn parses_vault_and_outbound_json() {
        let v = json!({
            "chain": "BTC",
            "vault": { "chain": "Bitcoin", "epoch": 1, "public_key": "02".to_string() + &"11".repeat(32), "chain_code": "22".repeat(32),
                       "signers": [Address::tagged(1).to_hex(), Address::tagged(2).to_hex()], "threshold": 2, "registered_height": 5 },
            "next_deposit_index": 3,
            "deposit_owners": { "1": Address::tagged(9).to_hex(), "2": Address::tagged(8).to_hex() }
        });
        let vault = parse_vault(Chain::Bitcoin, &v).unwrap().unwrap();
        assert_eq!(vault.epoch, 1);
        assert_eq!(vault.signers.len(), 2);
        assert_eq!(vault.owners[&2], Address::tagged(8));
        assert_eq!(vault.next_deposit_index, 3);
        assert!(parse_vault(Chain::Bitcoin, &json!({ "vault": null }))
            .unwrap()
            .is_none());

        let o = json!({ "outbounds": [
            { "id": 4, "owner": Address::tagged(9).to_hex(), "asset": "ETH.USDT", "chain": "Ethereum", "to": "0xabc", "amount": 1000000,
              "fee_asset": "ETH.ETH", "fee_estimate": "21000000000000", "status": "Batched", "batch_id": 2, "created_height": 10, "quorum": {"voters": []}, "tx_hash": null }
        ]});
        let rows = parse_outbounds(&o).unwrap();
        assert_eq!(rows[0].chain, Chain::Ethereum);
        assert_eq!(rows[0].fee_estimate, 21_000_000_000_000);
        assert_eq!(rows[0].batch_id, Some(2));
        assert_eq!(
            parse_params(&json!({ "confirmations_btc": 6 })).confirmations(Chain::Bitcoin),
            6
        );
        assert_eq!(
            parse_status(&json!({ "chain_id": 1, "height": "7" }))
                .unwrap()
                .height,
            7
        );
    }

    #[tokio::test]
    async fn submitter_tracks_nonce_and_idempotency() {
        let rpc = Arc::new(MockSttRpc {
            chain_id: CHAIN_ID_DEVNET,
            ..Default::default()
        });
        let state = Arc::new(StateFile::ephemeral());
        let key = Keypair::from_seed(3);
        let sub = Submitter::new(rpc.clone(), key, CHAIN_ID_DEVNET, state);
        let fee = Action::ReportNetworkFee {
            chain: Chain::Bitcoin,
            fee_rate: 5,
        };
        assert!(sub.submit(Some("k1"), fee.clone()).await.unwrap().admitted);
        assert!(sub
            .submit(Some("k1"), fee.clone())
            .await
            .unwrap()
            .tx_id
            .is_none());
        assert!(sub.submit(None, fee.clone()).await.unwrap().admitted);
        assert_eq!(rpc.submitted.lock().unwrap().len(), 2);
        assert_eq!(rpc.submitted.lock().unwrap()[1].envelope.nonce, 1);
        *rpc.refuse.lock().unwrap() = Some("boom".into());
        assert!(!sub.submit(Some("k2"), fee.clone()).await.unwrap().admitted);
        *rpc.refuse.lock().unwrap() = None;
        assert!(sub.submit(Some("k2"), fee).await.unwrap().admitted);
        assert_eq!(rpc.submitted.lock().unwrap()[2].envelope.nonce, 2);
    }
}
