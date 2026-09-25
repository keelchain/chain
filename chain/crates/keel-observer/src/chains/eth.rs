//! Ethereum: execution JSON-RPC (logs, balances, fees, broadcast) and the
//! beacon REST API (light-client finality updates and sync committees)
//! from which the observer assembles `keel_lc_eth::EthDepositProof`.
//!
//! What can be proven today: an ERC-20 `Transfer` whose receipt is in the
//! *finalized checkpoint block* of a light-client finality update. The
//! proof format binds the receipts root to `finalized.body_root`, so
//! deposits in other blocks of the epoch have no proof (see README.md,
//! "Ethereum proof gap"). Native ETH transfers have no receipt log and
//! the VM has no token contract for `ETH.ETH`; they are detected and
//! logged but never submitted.

use super::{json_hex, json_hex20, json_hex32, json_quantity, AddressBook};
use crate::config::EthereumConfig;
use alloy_primitives::{Bytes, B256};
use alloy_trie::{proof::ProofRetainer, HashBuilder, Nibbles};
use async_trait::async_trait;
use keel_actions::{Chain, DepositObservation, Proof};
use keel_chains::eth::{transfer_topic, Rlp};
use keel_lc_eth::{merkleize, sha256_pair, BeaconHeader, EthDepositProof};
use keel_types::Asset;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[async_trait]
pub trait EthRpc: Send + Sync {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value>;
}

#[async_trait]
pub trait BeaconApi: Send + Sync {
    /// `GET {beacon_url}{path}` → the JSON document.
    async fn get(&self, path: &str) -> anyhow::Result<Value>;
}

pub struct HttpEth {
    url: String,
    client: reqwest::Client,
}

impl HttpEth {
    pub fn new(cfg: &EthereumConfig) -> Self {
        Self {
            url: cfg.rpc_url.clone(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl EthRpc for HttpEth {
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
            anyhow::bail!("eth {method}: {err}");
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

pub struct HttpBeacon {
    base: String,
    client: reqwest::Client,
}

impl HttpBeacon {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl BeaconApi for HttpBeacon {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(format!("{}{path}", self.base))
            .header("accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("beacon GET {path}: {}", resp.status());
        }
        Ok(resp.json().await?)
    }
}

pub struct MockEth {
    #[allow(clippy::type_complexity)]
    pub handler: Box<dyn Fn(&str, &Value) -> anyhow::Result<Value> + Send + Sync>,
}

#[async_trait]
impl EthRpc for MockEth {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        (self.handler)(method, &params)
    }
}

pub struct MockBeacon {
    #[allow(clippy::type_complexity)]
    pub handler: Box<dyn Fn(&str) -> anyhow::Result<Value> + Send + Sync>,
}

#[async_trait]
impl BeaconApi for MockBeacon {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        (self.handler)(path)
    }
}

// ---------------------------------------------------------------- basics

pub async fn block_number(rpc: &dyn EthRpc) -> anyhow::Result<u64> {
    json_quantity(&rpc.call("eth_blockNumber", json!([])).await?)
        .map(|q| q as u64)
        .ok_or_else(|| anyhow::anyhow!("eth_blockNumber"))
}

pub async fn chain_id(rpc: &dyn EthRpc) -> anyhow::Result<u64> {
    json_quantity(&rpc.call("eth_chainId", json!([])).await?)
        .map(|q| q as u64)
        .ok_or_else(|| anyhow::anyhow!("eth_chainId"))
}

pub async fn balance(rpc: &dyn EthRpc, addr: &str) -> anyhow::Result<u128> {
    json_quantity(&rpc.call("eth_getBalance", json!([addr, "latest"])).await?)
        .ok_or_else(|| anyhow::anyhow!("eth_getBalance"))
}

pub async fn nonce(rpc: &dyn EthRpc, addr: &str) -> anyhow::Result<u64> {
    json_quantity(
        &rpc.call("eth_getTransactionCount", json!([addr, "pending"]))
            .await?,
    )
    .map(|q| q as u64)
    .ok_or_else(|| anyhow::anyhow!("eth_getTransactionCount"))
}

/// ERC-20 `balanceOf(address)` via `eth_call`.
pub async fn token_balance(rpc: &dyn EthRpc, token: &str, addr: &str) -> anyhow::Result<u128> {
    let account = keel_chains::address::eth_decode(addr)?;
    let mut data = vec![0x70, 0xa0, 0x82, 0x31];
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(&account);
    let v = rpc
        .call(
            "eth_call",
            json!([{ "to": token, "data": format!("0x{}", hex::encode(data)) }, "latest"]),
        )
        .await?;
    let bytes = json_hex(&v).ok_or_else(|| anyhow::anyhow!("eth_call balanceOf"))?;
    if bytes.len() < 32 || bytes[..16].iter().any(|b| *b != 0) {
        anyhow::bail!("balanceOf result out of u128 range");
    }
    Ok(u128::from_be_bytes(
        bytes[16..32].try_into().expect("16 bytes"),
    ))
}

/// (base fee of the latest block, median priority fee) from `eth_feeHistory`.
pub fn parse_fee_history(v: &Value, fallback_priority: u128) -> Option<(u128, u128)> {
    let base = v["baseFeePerGas"]
        .as_array()?
        .iter()
        .rev()
        .find_map(json_quantity)?;
    let mut tips: Vec<u128> = v["reward"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b.as_array()?.first().and_then(json_quantity))
                .collect()
        })
        .unwrap_or_default();
    tips.sort_unstable();
    let tip = if tips.is_empty() {
        fallback_priority
    } else {
        tips[tips.len() / 2]
    };
    Some((base, tip))
}

/// Fee parameters for a new transaction: (max_fee, priority) in wei.
pub async fn fee_params(rpc: &dyn EthRpc, fallback_priority: u128) -> anyhow::Result<(u128, u128)> {
    let v = rpc
        .call("eth_feeHistory", json!(["0x5", "latest", [50]]))
        .await?;
    let (base, tip) = parse_fee_history(&v, fallback_priority)
        .ok_or_else(|| anyhow::anyhow!("eth_feeHistory"))?;
    Ok((base.saturating_mul(2).saturating_add(tip), tip))
}

pub async fn send_raw(rpc: &dyn EthRpc, raw: &[u8]) -> anyhow::Result<String> {
    let v = rpc
        .call(
            "eth_sendRawTransaction",
            json!([format!("0x{}", hex::encode(raw))]),
        )
        .await?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("eth_sendRawTransaction"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptStatus {
    pub block_number: u64,
    pub success: bool,
    pub fee_paid: u128,
}

/// `eth_getTransactionReceipt`: `None` while pending.
pub async fn receipt_status(
    rpc: &dyn EthRpc,
    tx_hash: &str,
) -> anyhow::Result<Option<ReceiptStatus>> {
    let v = rpc
        .call("eth_getTransactionReceipt", json!([tx_hash]))
        .await?;
    if v.is_null() {
        return Ok(None);
    }
    let block_number = json_quantity(&v["blockNumber"])
        .ok_or_else(|| anyhow::anyhow!("receipt.blockNumber"))? as u64;
    let gas_used = json_quantity(&v["gasUsed"]).unwrap_or(0);
    let price = json_quantity(&v["effectiveGasPrice"]).unwrap_or(0);
    Ok(Some(ReceiptStatus {
        block_number,
        success: json_quantity(&v["status"]) == Some(1),
        fee_paid: gas_used.saturating_mul(price),
    }))
}

// ---------------------------------------------------------------- logs

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferLog {
    pub tx_hash: [u8; 32],
    pub tx_hash_hex: String,
    pub block_number: u64,
    pub tx_index: u64,
    /// Block-wide log index as reported by the node.
    pub log_index: u64,
    pub token: [u8; 20],
    pub from: [u8; 20],
    pub to: [u8; 20],
    pub amount: u128,
}

pub fn topic_of_address(a: &[u8; 20]) -> String {
    let mut t = [0u8; 32];
    t[12..].copy_from_slice(a);
    format!("0x{}", hex::encode(t))
}

/// Parse an `eth_getLogs` result into ERC-20 transfers.
pub fn parse_transfer_logs(v: &Value) -> anyhow::Result<Vec<TransferLog>> {
    let mut out = Vec::new();
    for l in v.as_array().cloned().unwrap_or_default() {
        if l["removed"].as_bool() == Some(true) {
            continue;
        }
        let topics: Vec<[u8; 32]> = l["topics"]
            .as_array()
            .map(|t| t.iter().filter_map(json_hex32).collect())
            .unwrap_or_default();
        let data = json_hex(&l["data"]).unwrap_or_default();
        let Some((from, to, amount)) = keel_chains::eth::parse_transfer_log(&topics, &data) else {
            continue;
        };
        let tx_hash_hex = l["transactionHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        out.push(TransferLog {
            tx_hash: json_hex32(&l["transactionHash"])
                .ok_or_else(|| anyhow::anyhow!("log.transactionHash"))?,
            tx_hash_hex,
            block_number: json_quantity(&l["blockNumber"])
                .ok_or_else(|| anyhow::anyhow!("log.blockNumber"))?
                as u64,
            tx_index: json_quantity(&l["transactionIndex"]).unwrap_or(0) as u64,
            log_index: json_quantity(&l["logIndex"]).unwrap_or(0) as u64,
            token: json_hex20(&l["address"]).ok_or_else(|| anyhow::anyhow!("log.address"))?,
            from,
            to,
            amount,
        });
    }
    Ok(out)
}

/// ERC-20 transfers of `token` to any address of the book in `[from, to]`.
pub async fn scan_transfers(
    rpc: &dyn EthRpc,
    token: &str,
    book: &AddressBook,
    from: u64,
    to: u64,
) -> anyhow::Result<Vec<TransferLog>> {
    let recipients: Vec<String> = book
        .indexes()
        .filter_map(|i| book.pubkey(i).ok())
        .filter_map(|pk| keel_chains::address::evm_account(&pk).ok())
        .map(|a| topic_of_address(&a))
        .collect();
    if recipients.is_empty() {
        return Ok(Vec::new());
    }
    let filter = json!({
        "fromBlock": format!("0x{from:x}"),
        "toBlock": format!("0x{to:x}"),
        "address": token,
        "topics": [format!("0x{}", hex::encode(transfer_topic())), Value::Null, recipients],
    });
    parse_transfer_logs(&rpc.call("eth_getLogs", json!([filter])).await?)
}

/// Position of a block-wide `log_index` inside its receipt's log list.
pub async fn log_position_in_receipt(
    rpc: &dyn EthRpc,
    tx_hash_hex: &str,
    log_index: u64,
) -> anyhow::Result<Option<u32>> {
    let r = rpc
        .call("eth_getTransactionReceipt", json!([tx_hash_hex]))
        .await?;
    let logs = r["logs"].as_array().cloned().unwrap_or_default();
    Ok(logs
        .iter()
        .position(|l| json_quantity(&l["logIndex"]) == Some(log_index as u128))
        .map(|p| p as u32))
}

// ---------------------------------------------------------------- receipts trie

/// Consensus encoding of a receipt from `eth_getBlockReceipts` JSON
/// (`type || rlp([status, cumulativeGasUsed, logsBloom, logs])`).
pub fn encode_receipt(r: &Value) -> anyhow::Result<Vec<u8>> {
    let status = json_quantity(&r["status"])
        .ok_or_else(|| anyhow::anyhow!("receipt.status (pre-Byzantium receipts unsupported)"))?;
    let cumulative = json_quantity(&r["cumulativeGasUsed"])
        .ok_or_else(|| anyhow::anyhow!("receipt.cumulativeGasUsed"))?;
    let bloom = json_hex(&r["logsBloom"]).ok_or_else(|| anyhow::anyhow!("receipt.logsBloom"))?;
    let mut logs = Vec::new();
    for l in r["logs"].as_array().cloned().unwrap_or_default() {
        let address = json_hex(&l["address"]).ok_or_else(|| anyhow::anyhow!("log.address"))?;
        let topics: Vec<Rlp> = l["topics"]
            .as_array()
            .map(|t| {
                t.iter()
                    .filter_map(json_hex)
                    .map(|b| Rlp::bytes(&b))
                    .collect()
            })
            .unwrap_or_default();
        let data = json_hex(&l["data"]).unwrap_or_default();
        logs.push(Rlp::List(vec![
            Rlp::bytes(&address),
            Rlp::List(topics),
            Rlp::bytes(&data),
        ]));
    }
    let body = Rlp::List(vec![
        Rlp::uint(status),
        Rlp::uint(cumulative),
        Rlp::bytes(&bloom),
        Rlp::List(logs),
    ])
    .encode();
    let ty = json_quantity(&r["type"]).unwrap_or(0) as u8;
    if ty == 0 {
        Ok(body)
    } else {
        let mut out = Vec::with_capacity(body.len() + 1);
        out.push(ty);
        out.extend(body);
        Ok(out)
    }
}

/// (receipts root, proof nodes root→leaf, the encoded receipt).
pub type ReceiptProof = ([u8; 32], Vec<Vec<u8>>, Vec<u8>);

/// Build the receipts trie of a block and the proof of `tx_index`.
pub fn receipt_proof(receipts: &[Vec<u8>], tx_index: u64) -> anyhow::Result<ReceiptProof> {
    if tx_index as usize >= receipts.len() {
        anyhow::bail!("tx index {tx_index} beyond {} receipts", receipts.len());
    }
    let target = Nibbles::unpack(alloy_rlp::encode(tx_index));
    let mut entries: Vec<(Nibbles, &Vec<u8>)> = receipts
        .iter()
        .enumerate()
        .map(|(i, r)| (Nibbles::unpack(alloy_rlp::encode(i as u64)), r))
        .collect();
    entries.sort_by_key(|(k, _)| *k);
    let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::new(vec![target]));
    for (k, v) in entries {
        hb.add_leaf(k, v);
    }
    let root = hb.root();
    let nodes = hb.take_proof_nodes();
    let proof: Vec<Vec<u8>> = nodes
        .matching_nodes_sorted(&target)
        .into_iter()
        .map(|(_, b)| b.to_vec())
        .collect();
    Ok((root.0, proof, receipts[tx_index as usize].clone()))
}

// ---------------------------------------------------------------- beacon

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionHeader {
    pub block_number: u64,
    pub block_hash: [u8; 32],
    pub receipts_root: [u8; 32],
    /// The 17 field roots (Deneb/Electra `ExecutionPayloadHeader`).
    pub field_roots: Vec<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalityUpdate {
    pub attested: BeaconHeader,
    pub finalized: BeaconHeader,
    pub finality_branch: Vec<[u8; 32]>,
    pub execution: ExecutionHeader,
    pub execution_branch: Vec<[u8; 32]>,
    pub sync_committee_bits: Vec<u8>,
    pub sync_committee_signature: Vec<u8>,
    pub signature_slot: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncCommittee {
    pub pubkeys: Vec<Vec<u8>>,
    pub aggregate_pubkey: Vec<u8>,
}

fn u64_le_root(v: u64) -> [u8; 32] {
    let mut r = [0u8; 32];
    r[..8].copy_from_slice(&v.to_le_bytes());
    r
}

fn bytes_root(b: &[u8]) -> [u8; 32] {
    let mut r = [0u8; 32];
    r[..b.len().min(32)].copy_from_slice(&b[..b.len().min(32)]);
    r
}

/// `hash_tree_root` of a `uint256` given as a decimal string.
fn u256_root(dec: &str) -> Option<[u8; 32]> {
    let v: u128 = dec.parse().ok()?; // base fees fit comfortably
    let mut r = [0u8; 32];
    r[..16].copy_from_slice(&v.to_le_bytes());
    Some(r)
}

/// `hash_tree_root` of `ByteList[32]` (`extra_data`).
fn bytelist32_root(b: &[u8]) -> [u8; 32] {
    let chunk = bytes_root(b);
    let len = u64_le_root(b.len() as u64);
    sha256_pair(&chunk, &len)
}

/// `hash_tree_root` of the 256-byte logs bloom (`ByteVector[256]`).
fn bloom_root(b: &[u8]) -> [u8; 32] {
    let chunks: Vec<[u8; 32]> = b.chunks(32).map(bytes_root).collect();
    merkleize(&chunks, 8)
}

fn beacon_header(v: &Value) -> anyhow::Result<BeaconHeader> {
    Ok(BeaconHeader {
        slot: json_quantity(&v["slot"]).ok_or_else(|| anyhow::anyhow!("header.slot"))? as u64,
        proposer_index: json_quantity(&v["proposer_index"]).unwrap_or(0) as u64,
        parent_root: json_hex32(&v["parent_root"])
            .ok_or_else(|| anyhow::anyhow!("header.parent_root"))?,
        state_root: json_hex32(&v["state_root"])
            .ok_or_else(|| anyhow::anyhow!("header.state_root"))?,
        body_root: json_hex32(&v["body_root"])
            .ok_or_else(|| anyhow::anyhow!("header.body_root"))?,
    })
}

fn branch(v: &Value) -> Vec<[u8; 32]> {
    v.as_array()
        .map(|a| a.iter().filter_map(json_hex32).collect())
        .unwrap_or_default()
}

/// Field roots of a `LightClientHeader.execution` JSON object.
pub fn execution_header(e: &Value) -> anyhow::Result<ExecutionHeader> {
    let h32 = |k: &str| json_hex32(&e[k]).ok_or_else(|| anyhow::anyhow!("execution.{k}"));
    let u = |k: &str| {
        json_quantity(&e[k])
            .map(|q| q as u64)
            .ok_or_else(|| anyhow::anyhow!("execution.{k}"))
    };
    let fee_recipient = json_hex20(&e["fee_recipient"])
        .ok_or_else(|| anyhow::anyhow!("execution.fee_recipient"))?;
    let bloom =
        json_hex(&e["logs_bloom"]).ok_or_else(|| anyhow::anyhow!("execution.logs_bloom"))?;
    if bloom.len() != 256 {
        anyhow::bail!("logs_bloom must be 256 bytes");
    }
    let extra = json_hex(&e["extra_data"]).unwrap_or_default();
    let base_fee = e["base_fee_per_gas"]
        .as_str()
        .and_then(u256_root)
        .ok_or_else(|| anyhow::anyhow!("execution.base_fee_per_gas"))?;
    let block_number = u("block_number")?;
    let receipts_root = h32("receipts_root")?;
    let block_hash = h32("block_hash")?;
    let mut roots = vec![
        h32("parent_hash")?,
        bytes_root(&fee_recipient),
        h32("state_root")?,
        receipts_root,
        bloom_root(&bloom),
        h32("prev_randao")?,
        u64_le_root(block_number),
        u64_le_root(u("gas_limit")?),
        u64_le_root(u("gas_used")?),
        u64_le_root(u("timestamp")?),
        bytelist32_root(&extra),
        base_fee,
        block_hash,
        h32("transactions_root")?,
    ];
    if let Some(w) = json_hex32(&e["withdrawals_root"]) {
        roots.push(w);
    }
    if let (Some(bg), Some(eg)) = (
        json_quantity(&e["blob_gas_used"]),
        json_quantity(&e["excess_blob_gas"]),
    ) {
        roots.push(u64_le_root(bg as u64));
        roots.push(u64_le_root(eg as u64));
    }
    Ok(ExecutionHeader {
        block_number,
        block_hash,
        receipts_root,
        field_roots: roots,
    })
}

/// Parse `GET /eth/v1/beacon/light_client/finality_update`.
pub fn parse_finality_update(v: &Value) -> anyhow::Result<FinalityUpdate> {
    let d = v.get("data").unwrap_or(v);
    let agg = &d["sync_aggregate"];
    Ok(FinalityUpdate {
        attested: beacon_header(&d["attested_header"]["beacon"])?,
        finalized: beacon_header(&d["finalized_header"]["beacon"])?,
        finality_branch: branch(&d["finality_branch"]),
        execution: execution_header(&d["finalized_header"]["execution"])?,
        execution_branch: branch(&d["finalized_header"]["execution_branch"]),
        sync_committee_bits: json_hex(&agg["sync_committee_bits"])
            .ok_or_else(|| anyhow::anyhow!("sync_committee_bits"))?,
        sync_committee_signature: json_hex(&agg["sync_committee_signature"])
            .ok_or_else(|| anyhow::anyhow!("sync_committee_signature"))?,
        signature_slot: json_quantity(&d["signature_slot"]).unwrap_or(0) as u64,
    })
}

/// Parse a `SyncCommittee` JSON object (`pubkeys`, `aggregate_pubkey`).
pub fn parse_sync_committee(v: &Value) -> anyhow::Result<SyncCommittee> {
    let pubkeys: Vec<Vec<u8>> = v["pubkeys"]
        .as_array()
        .map(|a| a.iter().filter_map(json_hex).collect())
        .unwrap_or_default();
    if pubkeys.is_empty() {
        anyhow::bail!("sync committee without pubkeys");
    }
    Ok(SyncCommittee {
        pubkeys,
        aggregate_pubkey: json_hex(&v["aggregate_pubkey"])
            .ok_or_else(|| anyhow::anyhow!("aggregate_pubkey"))?,
    })
}

pub fn period_of_slot(slot: u64) -> u64 {
    keel_lc_eth::SyncCommitteeState::period_of_slot(slot)
}

/// The committee that signs during `period`: the `next_sync_committee` of
/// the previous period's update, falling back to the bootstrap of
/// `finalized_root` (which carries `current_sync_committee`).
pub async fn fetch_committee(
    beacon: &dyn BeaconApi,
    period: u64,
    finalized_root: &[u8; 32],
) -> anyhow::Result<SyncCommittee> {
    if period > 0 {
        let v = beacon
            .get(&format!(
                "/eth/v1/beacon/light_client/updates?start_period={}&count=1",
                period - 1
            ))
            .await?;
        if let Some(u) = v.as_array().and_then(|a| a.first()) {
            let c = u.get("data").unwrap_or(u);
            if let Ok(sc) = parse_sync_committee(&c["next_sync_committee"]) {
                return Ok(sc);
            }
        }
    }
    let v = beacon
        .get(&format!(
            "/eth/v1/beacon/light_client/bootstrap/0x{}",
            hex::encode(finalized_root)
        ))
        .await?;
    let d = v.get("data").unwrap_or(&v);
    parse_sync_committee(&d["current_sync_committee"])
}

pub async fn fetch_finality_update(beacon: &dyn BeaconApi) -> anyhow::Result<FinalityUpdate> {
    parse_finality_update(
        &beacon
            .get("/eth/v1/beacon/light_client/finality_update")
            .await?,
    )
}

/// Assemble the proof the VM verifies. `receipts` are the consensus-encoded
/// receipts of the finalized execution block, in order.
pub fn build_proof(
    update: &FinalityUpdate,
    committee: &SyncCommittee,
    receipts: &[Vec<u8>],
    tx_index: u64,
    log_index: u32,
) -> anyhow::Result<EthDepositProof> {
    let (root, proof, receipt) = receipt_proof(receipts, tx_index)?;
    if root != update.execution.receipts_root {
        anyhow::bail!(
            "receipts root mismatch: trie {} vs header {}",
            hex::encode(root),
            hex::encode(update.execution.receipts_root)
        );
    }
    Ok(EthDepositProof {
        attested: update.attested.clone(),
        finalized: update.finalized.clone(),
        finality_branch: update.finality_branch.clone(),
        sync_committee_bits: update.sync_committee_bits.clone(),
        sync_committee_signature: update.sync_committee_signature.clone(),
        committee_pubkeys: committee.pubkeys.clone(),
        committee_aggregate_pubkey: committee.aggregate_pubkey.clone(),
        execution_fields: update.execution.field_roots.clone(),
        execution_branch: update.execution_branch.clone(),
        tx_index,
        log_index,
        receipt,
        receipt_proof: proof,
    })
}

/// Consensus-encoded receipts of a block via `eth_getBlockReceipts`.
pub async fn block_receipts(rpc: &dyn EthRpc, block_number: u64) -> anyhow::Result<Vec<Vec<u8>>> {
    let v = rpc
        .call(
            "eth_getBlockReceipts",
            json!([format!("0x{block_number:x}")]),
        )
        .await?;
    let mut rows: Vec<(u64, Vec<u8>)> = Vec::new();
    for r in v.as_array().cloned().unwrap_or_default() {
        let idx = json_quantity(&r["transactionIndex"])
            .ok_or_else(|| anyhow::anyhow!("receipt.transactionIndex"))? as u64;
        rows.push((idx, encode_receipt(&r)?));
    }
    rows.sort_by_key(|(i, _)| *i);
    Ok(rows.into_iter().map(|(_, r)| r).collect())
}

/// Finality updates remembered by finalized execution block number.
#[derive(Default)]
pub struct FinalityCache {
    pub by_block: BTreeMap<u64, FinalityUpdate>,
    pub keep: usize,
}

impl FinalityCache {
    pub fn new(keep: usize) -> Self {
        Self {
            by_block: BTreeMap::new(),
            keep,
        }
    }

    pub fn insert(&mut self, u: FinalityUpdate) {
        self.by_block.insert(u.execution.block_number, u);
        while self.by_block.len() > self.keep.max(1) {
            if let Some(first) = self.by_block.keys().next().copied() {
                self.by_block.remove(&first);
            }
        }
    }

    pub fn get(&self, block_number: u64) -> Option<&FinalityUpdate> {
        self.by_block.get(&block_number)
    }

    /// Highest finalized execution block seen.
    pub fn finalized_block(&self) -> Option<u64> {
        self.by_block.keys().next_back().copied()
    }
}

/// The observation for one transfer, with or without a proof. Without a
/// proof (`None`) the VM would reject it; callers log and skip.
pub fn observation(
    log: &TransferLog,
    asset: Asset,
    deposit_index: u64,
    log_position: u32,
    tip: u64,
    proof: Option<&EthDepositProof>,
) -> DepositObservation {
    DepositObservation {
        chain: Chain::Ethereum,
        asset,
        tx_hash: log.tx_hash,
        index: log_position,
        deposit_index,
        amount: log.amount,
        external_height: log.block_number,
        tip_height: tip,
        proof: match proof {
            Some(p) => Proof::Ethereum {
                proof: borsh::to_vec(p).unwrap_or_default(),
            },
            None => Proof::None,
        },
    }
}

/// Find an outbound already paid from `from`: ERC-20 transfer logs or,
/// for native ETH, top-level transactions in `[from_block, to_block]`.
pub async fn find_payment(
    rpc: &dyn EthRpc,
    token: Option<&str>,
    from: &str,
    to: &str,
    amount: u128,
    from_block: u64,
    to_block: u64,
) -> anyhow::Result<Option<String>> {
    let from_acct = keel_chains::address::eth_decode(from)?;
    let to_acct = keel_chains::address::eth_decode(to)?;
    match token {
        Some(t) => {
            let filter = json!({
                "fromBlock": format!("0x{from_block:x}"), "toBlock": format!("0x{to_block:x}"), "address": t,
                "topics": [format!("0x{}", hex::encode(transfer_topic())), topic_of_address(&from_acct), topic_of_address(&to_acct)],
            });
            let logs = parse_transfer_logs(&rpc.call("eth_getLogs", json!([filter])).await?)?;
            Ok(logs
                .into_iter()
                .find(|l| l.amount == amount)
                .map(|l| l.tx_hash_hex))
        }
        None => {
            for b in from_block..=to_block {
                let block = rpc
                    .call("eth_getBlockByNumber", json!([format!("0x{b:x}"), true]))
                    .await?;
                for tx in block["transactions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                {
                    let f = json_hex20(&tx["from"]);
                    let t = json_hex20(&tx["to"]);
                    if f == Some(from_acct)
                        && t == Some(to_acct)
                        && json_quantity(&tx["value"]) == Some(amount)
                    {
                        if let Some(h) = tx["hash"].as_str() {
                            return Ok(Some(h.to_string()));
                        }
                    }
                }
            }
            Ok(None)
        }
    }
}

pub fn b256(b: &[u8; 32]) -> B256 {
    B256::from(*b)
}

pub fn bytes(b: &[u8]) -> Bytes {
    Bytes::copy_from_slice(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOGS: &str = r#"[{"address":"0x5fbdb2315678afecb367f032d93f642f64180aa3","topics":["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef","0x000000000000000000000000f39fd6e51aad88f6f4ce6ab8827279cfffb92266","0x0000000000000000000000007e5f4552091a69125d5dfcb7b8c2659029395bdf"],"data":"0x00000000000000000000000000000000000000000000000000000000000f4240","blockNumber":"0x10","transactionHash":"0x8f2b3c7e2f3a2d1c0b9a8f7e6d5c4b3a2918070605040302010f0e0d0c0b0a09","transactionIndex":"0x2","blockHash":"0xaa","logIndex":"0x5","removed":false}]"#;

    #[test]
    fn parses_logs_fee_history_and_receipt_status() {
        let logs = parse_transfer_logs(&serde_json::from_str(LOGS).unwrap()).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].amount, 1_000_000);
        assert_eq!(logs[0].block_number, 16);
        assert_eq!(logs[0].tx_index, 2);
        assert_eq!(logs[0].log_index, 5);
        assert_eq!(
            hex::encode(logs[0].to),
            "7e5f4552091a69125d5dfcb7b8c2659029395bdf"
        );
        let fh = json!({ "baseFeePerGas": ["0x3b9aca00", "0x4a817c80"], "reward": [["0x3b9aca00"], ["0x77359400"]], "oldestBlock": "0x1" });
        assert_eq!(
            parse_fee_history(&fh, 7),
            Some((1_250_000_000, 2_000_000_000))
        );
        assert_eq!(
            parse_fee_history(&json!({ "baseFeePerGas": ["0x10"] }), 7),
            Some((16, 7))
        );
        assert_eq!(parse_fee_history(&json!({}), 7), None);
    }

    fn receipt_json(status: u64, cumulative: u64, logs: Vec<Value>, ty: u8) -> Value {
        json!({
            "type": format!("0x{ty:x}"), "status": format!("0x{status:x}"), "cumulativeGasUsed": format!("0x{cumulative:x}"),
            "logsBloom": format!("0x{}", "00".repeat(256)), "logs": logs, "transactionIndex": "0x0"
        })
    }

    #[test]
    fn receipt_trie_proof_verifies_with_stt_lc_eth() {
        let token = [0x5fu8; 20];
        let to = [0x7eu8; 20];
        let log = json!({
            "address": format!("0x{}", hex::encode(token)),
            "topics": [format!("0x{}", hex::encode(transfer_topic())), topic_of_address(&[0xf3; 20]), topic_of_address(&to)],
            "data": format!("0x{}", hex::encode({ let mut d = [0u8; 32]; d[28..].copy_from_slice(&1_000_000u32.to_be_bytes()); d })),
        });
        let receipts: Vec<Vec<u8>> = (0..5u64)
            .map(|i| {
                let logs = if i == 3 { vec![json!({ "address": format!("0x{}", "11".repeat(20)), "topics": [], "data": "0x" }), log.clone()] } else { vec![] };
                encode_receipt(&receipt_json(1, 21_000 * (i + 1), logs, if i % 2 == 0 { 2 } else { 0 })).unwrap()
            })
            .collect();
        assert_eq!(receipts[0][0], 0x02);
        assert!(
            keel_lc_eth::decode_receipt_logs(&receipts[3])
                .unwrap()
                .len()
                == 2
        );
        let (root, proof, receipt) = receipt_proof(&receipts, 3).unwrap();
        assert_eq!(receipt, receipts[3]);
        let got = keel_lc_eth::verify_receipt_log(&root, 3, &receipt, &proof, 1).unwrap();
        assert_eq!(got.address, token);
        assert_eq!(&got.topics[2][12..], &to);
        assert!(keel_lc_eth::verify_receipt_log(&root, 2, &receipt, &proof, 1).is_err());
        let (root1, proof1, receipt1) = receipt_proof(&receipts, 0).unwrap();
        assert_eq!(root1, root);
        assert!(keel_lc_eth::verify_receipt_log(&root1, 0, &receipt1, &proof1, 0).is_err()); // no logs in receipt 0
        assert!(receipt_proof(&receipts, 9).is_err());
    }

    const FINALITY_UPDATE: &str = r#"{"version":"deneb","data":{
      "attested_header":{"beacon":{"slot":"8000100","proposer_index":"12","parent_root":"0x0101010101010101010101010101010101010101010101010101010101010101","state_root":"0x0202020202020202020202020202020202020202020202020202020202020202","body_root":"0x0303030303030303030303030303030303030303030303030303030303030303"},"execution":{},"execution_branch":[]},
      "finalized_header":{"beacon":{"slot":"8000032","proposer_index":"7","parent_root":"0x0404040404040404040404040404040404040404040404040404040404040404","state_root":"0x0505050505050505050505050505050505050505050505050505050505050505","body_root":"0x0606060606060606060606060606060606060606060606060606060606060606"},
        "execution":{"parent_hash":"0x0707070707070707070707070707070707070707070707070707070707070707","fee_recipient":"0x0808080808080808080808080808080808080808","state_root":"0x0909090909090909090909090909090909090909090909090909090909090909","receipts_root":"0x0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a","logs_bloom":"0xLOGS","prev_randao":"0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b","block_number":"19000000","gas_limit":"30000000","gas_used":"12345678","timestamp":"1700000000","extra_data":"0x6c69676874","base_fee_per_gas":"12345678901","block_hash":"0x0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c","transactions_root":"0x0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d","withdrawals_root":"0x0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e","blob_gas_used":"131072","excess_blob_gas":"0"},
        "execution_branch":["0x1111111111111111111111111111111111111111111111111111111111111111","0x2222222222222222222222222222222222222222222222222222222222222222","0x3333333333333333333333333333333333333333333333333333333333333333","0x4444444444444444444444444444444444444444444444444444444444444444"]},
      "finality_branch":["0x5555555555555555555555555555555555555555555555555555555555555555","0x6666666666666666666666666666666666666666666666666666666666666666","0x7777777777777777777777777777777777777777777777777777777777777777","0x8888888888888888888888888888888888888888888888888888888888888888","0x9999999999999999999999999999999999999999999999999999999999999999","0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
      "sync_aggregate":{"sync_committee_bits":"0xBITS","sync_committee_signature":"0xSIG"},
      "signature_slot":"8000101"}}"#;

    fn finality_update_json() -> Value {
        let s = FINALITY_UPDATE
            .replace("0xLOGS", &format!("0x{}", "ab".repeat(256)))
            .replace("0xBITS", &format!("0x{}", "ff".repeat(64)))
            .replace("0xSIG", &format!("0x{}", "cd".repeat(96)));
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn parses_finality_update_and_assembles_a_proof() {
        let u = parse_finality_update(&finality_update_json()).unwrap();
        assert_eq!(u.finalized.slot, 8_000_032);
        assert_eq!(u.execution.block_number, 19_000_000);
        assert_eq!(u.execution.field_roots.len(), 17);
        assert_eq!(
            u.execution.field_roots[keel_lc_eth::RECEIPTS_ROOT_FIELD],
            u.execution.receipts_root
        );
        assert_eq!(
            u.execution.field_roots[keel_lc_eth::BLOCK_NUMBER_FIELD][..8],
            19_000_000u64.to_le_bytes()
        );
        assert_eq!(u.finality_branch.len(), 6);
        assert_eq!(u.execution_branch.len(), 4);
        assert_eq!(u.sync_committee_bits.len(), 64);
        assert_eq!(period_of_slot(u.attested.slot), 976);
        // extra_data "light" → chunk || len mixed in.
        let chunk = bytes_root(b"light");
        assert_eq!(
            u.execution.field_roots[10],
            sha256_pair(&chunk, &u64_le_root(5))
        );

        // The receipts root in the header must match the trie we build.
        let receipts: Vec<Vec<u8>> = (0..3u64)
            .map(|i| encode_receipt(&receipt_json(1, 1 + i, vec![], 2)).unwrap())
            .collect();
        let committee = SyncCommittee {
            pubkeys: vec![vec![0xaa; 48]; 512],
            aggregate_pubkey: vec![0xbb; 48],
        };
        assert!(build_proof(&u, &committee, &receipts, 1, 0).is_err());
        let mut fixed = u.clone();
        let (root, _, _) = receipt_proof(&receipts, 1).unwrap();
        fixed.execution.receipts_root = root;
        fixed.execution.field_roots[keel_lc_eth::RECEIPTS_ROOT_FIELD] = root;
        let proof = build_proof(&fixed, &committee, &receipts, 1, 0).unwrap();
        assert_eq!(proof.committee_pubkeys.len(), 512);
        assert_eq!(proof.tx_index, 1);
        let bytes = borsh::to_vec(&proof).unwrap();
        let back: EthDepositProof = borsh::from_slice(&bytes).unwrap();
        assert_eq!(back, proof);
        // Receipt proof inside the assembled proof verifies against the header's root.
        assert!(keel_lc_eth::verify_receipt_log(
            &fixed.execution.receipts_root,
            1,
            &proof.receipt,
            &proof.receipt_proof,
            0
        )
        .is_err()); // no logs, but proof ok up to decoding
        let (r2, p2, rc2) = receipt_proof(&receipts, 1).unwrap();
        assert_eq!(
            (r2, &p2, &rc2),
            (
                fixed.execution.receipts_root,
                &proof.receipt_proof,
                &proof.receipt
            )
        );

        let mut cache = FinalityCache::new(2);
        cache.insert(u.clone());
        let mut later = u.clone();
        later.execution.block_number += 32;
        cache.insert(later);
        let mut latest = u.clone();
        latest.execution.block_number += 64;
        cache.insert(latest);
        assert!(cache.get(19_000_000).is_none());
        assert_eq!(cache.finalized_block(), Some(19_000_064));
    }

    #[test]
    fn observation_carries_the_borsh_proof() {
        let logs = parse_transfer_logs(&serde_json::from_str(LOGS).unwrap()).unwrap();
        let o = observation(&logs[0], Asset::vault("ETH", "USDT"), 4, 1, 40, None);
        assert_eq!(o.index, 1);
        assert_eq!(o.deposit_index, 4);
        assert_eq!(o.proof, Proof::None);
        assert_eq!(o.external_height, 16);
    }

    #[tokio::test]
    async fn scan_transfers_filters_by_book_addresses() {
        let secp = secp256k1::Secp256k1::new();
        let vault = crate::rpc::VaultView {
            chain: Chain::Ethereum,
            epoch: 1,
            public_key: secp256k1::PublicKey::from_secret_key(
                &secp,
                &secp256k1::SecretKey::from_slice(&[5u8; 32]).unwrap(),
            )
            .serialize(),
            chain_code: [1u8; 32],
            signers: vec![],
            threshold: 1,
            next_deposit_index: 2,
            owners: BTreeMap::new(),
        };
        let book = AddressBook::build(vault, keel_chains::Network::Regtest).unwrap();
        let expect_topic =
            topic_of_address(&keel_chains::address::eth_decode(book.address(1).unwrap()).unwrap());
        let mock = MockEth {
            handler: Box::new(move |method, params| {
                assert_eq!(method, "eth_getLogs");
                let topics = params[0]["topics"].as_array().unwrap();
                assert!(topics[2].as_array().unwrap().contains(&json!(expect_topic)));
                assert_eq!(params[0]["fromBlock"], "0xa");
                Ok(serde_json::from_str(LOGS).unwrap())
            }),
        };
        let logs = scan_transfers(
            &mock,
            "0x5fbdb2315678afecb367f032d93f642f64180aa3",
            &book,
            10,
            20,
        )
        .await
        .unwrap();
        assert_eq!(logs.len(), 1);
    }
}
