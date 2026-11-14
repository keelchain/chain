//! Solana JSON-RPC: deposits read from finalized transactions' balance
//! deltas (attestation only, `Proof::None`), transaction building via
//! `keel_chains::sol`, broadcast and confirmation tracking.
//!
//! The chain keys a deposit by `(chain, tx_hash, index)` with a 32-byte
//! hash, and a Solana transaction id is a 64-byte signature: the hash the
//! chain sees is `SHA-256(signature)`; `index` is the account's position
//! in the transaction (the wallet for SOL, its token account for SPL).

use super::{json_quantity, AddressBook};
use crate::config::SolanaConfig;
use async_trait::async_trait;
use base64::Engine as _;
use keel_actions::{Chain, DepositObservation, Proof};
use keel_chains::sol;
use keel_types::Asset;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[async_trait]
pub trait SolRpc: Send + Sync {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value>;
}

pub struct HttpSol {
    url: String,
    client: reqwest::Client,
}

impl HttpSol {
    pub fn new(cfg: &SolanaConfig) -> Self {
        Self {
            url: cfg.rpc_url.clone(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl SolRpc for HttpSol {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let v: Value = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            anyhow::bail!("solana {method}: {err}");
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// Fixture handler of [`MockSol`]: `(method, params)` → result.
pub type SolHandler = Box<dyn Fn(&str, &Value) -> anyhow::Result<Value> + Send + Sync>;

pub struct MockSol {
    pub handler: SolHandler,
}

#[async_trait]
impl SolRpc for MockSol {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        (self.handler)(method, &params)
    }
}

const FINALIZED: &str = "finalized";

/// The chain's 32-byte id of a Solana transaction.
pub fn tx_hash_of(signature: &[u8; 64]) -> [u8; 32] {
    keel_crypto::sha256(&[signature])
}

pub fn decode_signature(sig: &str) -> anyhow::Result<[u8; 64]> {
    bs58::decode(sig)
        .into_vec()?
        .try_into()
        .map_err(|_| anyhow::anyhow!("signature is not 64 bytes"))
}

pub async fn slot(rpc: &dyn SolRpc) -> anyhow::Result<u64> {
    json_quantity(
        &rpc.call("getSlot", json!([{ "commitment": FINALIZED }]))
            .await?,
    )
    .map(|v| v as u64)
    .ok_or_else(|| anyhow::anyhow!("getSlot"))
}

pub async fn block_height(rpc: &dyn SolRpc) -> anyhow::Result<u64> {
    json_quantity(
        &rpc.call("getBlockHeight", json!([{ "commitment": FINALIZED }]))
            .await?,
    )
    .map(|v| v as u64)
    .ok_or_else(|| anyhow::anyhow!("getBlockHeight"))
}

/// `(blockhash, last valid block height)`.
pub async fn latest_blockhash(rpc: &dyn SolRpc) -> anyhow::Result<([u8; 32], u64)> {
    let v = rpc
        .call("getLatestBlockhash", json!([{ "commitment": FINALIZED }]))
        .await?;
    let hash = v["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("getLatestBlockhash"))?;
    let hash: [u8; 32] = bs58::decode(hash)
        .into_vec()?
        .try_into()
        .map_err(|_| anyhow::anyhow!("blockhash length"))?;
    let last = json_quantity(&v["value"]["lastValidBlockHeight"]).unwrap_or(0) as u64;
    Ok((hash, last))
}

pub async fn balance(rpc: &dyn SolRpc, addr: &str) -> anyhow::Result<u64> {
    let v = rpc
        .call("getBalance", json!([addr, { "commitment": FINALIZED }]))
        .await?;
    Ok(json_quantity(&v["value"]).unwrap_or(0) as u64)
}

/// Balance of a token account, 0 when it does not exist.
pub async fn token_account_balance(rpc: &dyn SolRpc, account: &str) -> anyhow::Result<u64> {
    match rpc
        .call(
            "getTokenAccountBalance",
            json!([account, { "commitment": FINALIZED }]),
        )
        .await
    {
        Ok(v) => Ok(v["value"]["amount"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)),
        // A missing account answers with an error; it holds nothing.
        Err(e)
            if e.to_string().contains("could not find account")
                || e.to_string().contains("-32602") =>
        {
            Ok(0)
        }
        Err(e) => Err(e),
    }
}

/// Decimals of a mint (`getTokenSupply`).
pub async fn mint_decimals(rpc: &dyn SolRpc, mint: &str) -> anyhow::Result<u8> {
    let v = rpc
        .call("getTokenSupply", json!([mint, { "commitment": FINALIZED }]))
        .await?;
    json_quantity(&v["value"]["decimals"])
        .map(|d| d as u8)
        .ok_or_else(|| anyhow::anyhow!("getTokenSupply: no decimals"))
}

pub fn ata(owner: &str, mint: &str) -> anyhow::Result<String> {
    let o = sol::decode(owner)?;
    let m = sol::decode(mint)?;
    Ok(sol::address(&sol::associated_token_account(&o, &m)?))
}

/// Newest signatures touching `addr` (finalized), newest first.
pub async fn signatures(rpc: &dyn SolRpc, addr: &str, limit: u64) -> anyhow::Result<Vec<SigInfo>> {
    let v = rpc
        .call(
            "getSignaturesForAddress",
            json!([addr, { "limit": limit, "commitment": FINALIZED }]),
        )
        .await?;
    Ok(v.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            Some(SigInfo {
                signature: e["signature"].as_str()?.to_string(),
                slot: json_quantity(&e["slot"])? as u64,
                failed: !e["err"].is_null(),
            })
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigInfo {
    pub signature: String,
    pub slot: u64,
    pub failed: bool,
}

/// A finalized transaction as the balance deltas describe it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TxView {
    pub slot: u64,
    pub success: bool,
    pub fee: u64,
    pub account_keys: Vec<String>,
    pub pre: Vec<u64>,
    pub post: Vec<u64>,
    /// (account index, owner, mint, pre, post) of every token balance.
    pub tokens: Vec<(u32, String, String, u64, u64)>,
}

impl TxView {
    pub fn lamport_delta(&self, addr: &str) -> Option<(u32, i128)> {
        let i = self.account_keys.iter().position(|k| k == addr)?;
        Some((
            i as u32,
            self.post.get(i).copied()? as i128 - self.pre.get(i).copied()? as i128,
        ))
    }

    /// (account index, mint, delta) of every token balance `owner` holds.
    pub fn token_deltas(&self, owner: &str) -> Vec<(u32, String, i128)> {
        self.tokens
            .iter()
            .filter(|(_, o, _, _, _)| o == owner)
            .map(|(i, _, mint, pre, post)| (*i, mint.clone(), *post as i128 - *pre as i128))
            .collect()
    }
}

pub fn parse_transaction(v: &Value) -> Option<TxView> {
    if v.is_null() {
        return None;
    }
    let meta = &v["meta"];
    let mut account_keys: Vec<String> = v["transaction"]["message"]["accountKeys"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|k| {
                    k.as_str()
                        .or_else(|| k["pubkey"].as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    for group in ["writable", "readonly"] {
        if let Some(more) = meta["loadedAddresses"][group].as_array() {
            account_keys.extend(more.iter().filter_map(|k| k.as_str().map(str::to_string)));
        }
    }
    let nums = |x: &Value| -> Vec<u64> {
        x.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| json_quantity(n).map(|q| q as u64))
                    .collect()
            })
            .unwrap_or_default()
    };
    let token_rows = |x: &Value| -> BTreeMap<u32, (String, String, u64)> {
        x.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some((
                            json_quantity(&t["accountIndex"])? as u32,
                            (
                                t["owner"].as_str().unwrap_or_default().to_string(),
                                t["mint"].as_str()?.to_string(),
                                t["uiTokenAmount"]["amount"].as_str()?.parse().ok()?,
                            ),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let pre_t = token_rows(&meta["preTokenBalances"]);
    let post_t = token_rows(&meta["postTokenBalances"]);
    let mut tokens = Vec::new();
    for (i, (owner, mint, post)) in &post_t {
        let pre = pre_t.get(i).map(|(_, _, p)| *p).unwrap_or(0);
        tokens.push((*i, owner.clone(), mint.clone(), pre, *post));
    }
    for (i, (owner, mint, pre)) in &pre_t {
        if !post_t.contains_key(i) {
            tokens.push((*i, owner.clone(), mint.clone(), *pre, 0));
        }
    }
    Some(TxView {
        slot: json_quantity(&v["slot"]).unwrap_or(0) as u64,
        success: meta["err"].is_null(),
        fee: json_quantity(&meta["fee"]).unwrap_or(0) as u64,
        account_keys,
        pre: nums(&meta["preBalances"]),
        post: nums(&meta["postBalances"]),
        tokens,
    })
}

pub async fn transaction(rpc: &dyn SolRpc, signature: &str) -> anyhow::Result<Option<TxView>> {
    let v = rpc
        .call(
            "getTransaction",
            json!([signature, { "encoding": "json", "commitment": FINALIZED, "maxSupportedTransactionVersion": 0 }]),
        )
        .await?;
    Ok(parse_transaction(&v))
}

/// `(slot, finalized, failed)` of a signature, `None` when unknown.
pub async fn signature_status(
    rpc: &dyn SolRpc,
    signature: &str,
) -> anyhow::Result<Option<(u64, bool, bool)>> {
    let v = rpc
        .call(
            "getSignatureStatuses",
            json!([[signature], { "searchTransactionHistory": true }]),
        )
        .await?;
    let s = &v["value"][0];
    if s.is_null() {
        return Ok(None);
    }
    Ok(Some((
        json_quantity(&s["slot"]).unwrap_or(0) as u64,
        s["confirmationStatus"].as_str() == Some(FINALIZED),
        !s["err"].is_null(),
    )))
}

pub async fn send_transaction(rpc: &dyn SolRpc, raw: &[u8]) -> anyhow::Result<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
    let v = rpc
        .call(
            "sendTransaction",
            json!([encoded, { "encoding": "base64", "maxRetries": 3, "preflightCommitment": "confirmed" }]),
        )
        .await?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("sendTransaction returned no signature"))
}

/// A deposit a transaction made to a book address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolDeposit {
    pub deposit_index: u64,
    /// Position of the credited account in the transaction.
    pub account_index: u32,
    /// `None` for SOL, the mint for a token.
    pub mint: Option<String>,
    pub amount: u64,
}

/// Every positive balance change a transaction made to a watched address.
pub fn deposits_in(view: &TxView, book: &AddressBook) -> Vec<SolDeposit> {
    let mut out = Vec::new();
    if !view.success {
        return out;
    }
    for (index, addr) in book.addresses() {
        if !book.watches(index) {
            continue;
        }
        if let Some((i, delta)) = view.lamport_delta(addr) {
            if delta > 0 {
                out.push(SolDeposit {
                    deposit_index: index,
                    account_index: i,
                    mint: None,
                    amount: delta as u64,
                });
            }
        }
        for (i, mint, delta) in view.token_deltas(addr) {
            if delta > 0 {
                out.push(SolDeposit {
                    deposit_index: index,
                    account_index: i,
                    mint: Some(mint),
                    amount: delta as u64,
                });
            }
        }
    }
    out
}

pub fn observation(
    d: &SolDeposit,
    asset: Asset,
    signature: &[u8; 64],
    signature_b58: &str,
    slot: u64,
    tip: u64,
) -> DepositObservation {
    DepositObservation {
        external_tx: Some(signature_b58.to_string()),
        chain: Chain::Solana,
        asset,
        tx_hash: tx_hash_of(signature),
        index: d.account_index,
        deposit_index: d.deposit_index,
        amount: d.amount as u128,
        external_height: slot,
        tip_height: tip,
        proof: Proof::None,
    }
}

/// Lamports a message will cost its fee payer: base fees per signature
/// plus the priority fee at the configured price.
pub fn fee_estimate(cfg: &SolanaConfig, signatures: u64) -> u64 {
    sol::LAMPORTS_PER_SIGNATURE * signatures.max(1)
        + cfg.priority_micro_lamports.saturating_mul(200_000) / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::VaultView;
    use curve25519_dalek::scalar::Scalar;

    fn book() -> (AddressBook, [u8; 32]) {
        let x = Scalar::from_bytes_mod_order([21u8; 32]);
        let vault = sol::pubkey_of_scalar(&x);
        let v = VaultView {
            chain: Chain::Solana,
            epoch: 1,
            public_key: vault.to_vec(),
            chain_code: [4u8; 32],
            signers: vec![],
            threshold: 1,
            next_deposit_index: 3,
            owners: BTreeMap::from([
                (1, keel_types::Address::tagged(1)),
                (2, keel_types::Address::tagged(2)),
            ]),
            custodian: None,
            signer_url: None,
            gen: 0,
            status: "Active".into(),
            sweep: false,
            drain_until: 0,
            swept_any: false,
            pending_sweeps: Vec::new(),
        };
        (
            AddressBook::build(v, keel_chains::Network::Testnet).unwrap(),
            vault,
        )
    }

    #[test]
    fn deposits_come_from_balance_deltas() {
        let (book, _) = book();
        let a1 = book.address(1).unwrap().to_string();
        let a2 = book.address(2).unwrap().to_string();
        let mint = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
        let ata1 = ata(&a1, mint).unwrap();
        let v = json!({
            "slot": 500,
            "meta": {
                "err": null, "fee": 5000,
                "preBalances": [10_000_000, 0, 1_000],
                "postBalances": [9_000_000, 995_000, 1_000],
                "preTokenBalances": [],
                "postTokenBalances": [{ "accountIndex": 3, "mint": mint, "owner": a1, "uiTokenAmount": { "amount": "2500000" } }],
                "loadedAddresses": { "writable": [ata1], "readonly": [] }
            },
            "transaction": { "message": { "accountKeys": ["payer111", a1, a2] } }
        });
        let view = parse_transaction(&v).unwrap();
        assert_eq!(view.account_keys.len(), 4);
        assert_eq!(view.lamport_delta(&a1), Some((1, 995_000)));
        assert_eq!(view.lamport_delta(&a2), Some((2, 0)));
        let d = deposits_in(&view, &book);
        assert_eq!(d.len(), 2);
        assert_eq!(
            d[0],
            SolDeposit {
                deposit_index: 1,
                account_index: 1,
                mint: None,
                amount: 995_000
            }
        );
        assert_eq!(
            d[1],
            SolDeposit {
                deposit_index: 1,
                account_index: 3,
                mint: Some(mint.into()),
                amount: 2_500_000
            }
        );
        let sig = [7u8; 64];
        let o = observation(
            &d[1],
            Asset::vault("SOL", "USDC"),
            &sig,
            &bs58::encode(sig).into_string(),
            500,
            540,
        );
        assert_eq!(o.chain, Chain::Solana);
        assert_eq!(o.index, 3);
        assert_eq!(o.tx_hash, tx_hash_of(&sig));
        assert_eq!(
            o.external_tx.as_deref(),
            Some(bs58::encode(sig).into_string().as_str())
        );
        assert_eq!(o.proof, Proof::None);
        // The hot address (index 0) is not watched on the network vault.
        let mut failed = view.clone();
        failed.success = false;
        assert!(deposits_in(&failed, &book).is_empty());
        assert_eq!(parse_transaction(&Value::Null), None);
    }
}
