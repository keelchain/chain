//! Tron full-node HTTP API: TRC-20 and TRX deposits (attestation only,
//! `Proof::None`), transaction building via `keel_chains::tron`, broadcast
//! and confirmation tracking.

use super::{json_hex, json_quantity, AddressBook};
use crate::config::TronConfig;
use async_trait::async_trait;
use keel_actions::{Chain, DepositObservation, Proof};
use keel_chains::address::{tron_address_from_account, tron_decode};
use keel_types::Asset;
use serde_json::{json, Value};

#[async_trait]
pub trait TronApi: Send + Sync {
    async fn get(&self, path: &str) -> anyhow::Result<Value>;
    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value>;
}

pub struct HttpTron {
    base: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl HttpTron {
    pub fn new(cfg: &TronConfig) -> Self {
        Self {
            base: cfg.api_url.trim_end_matches('/').to_string(),
            api_key: cfg.api_key.clone(),
            client: reqwest::Client::new(),
        }
    }

    fn req(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) => r.header("TRON-PRO-API-KEY", k),
            None => r,
        }
    }
}

#[async_trait]
impl TronApi for HttpTron {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        Ok(self
            .req(self.client.get(format!("{}{path}", self.base)))
            .send()
            .await?
            .json()
            .await?)
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        Ok(self
            .req(self.client.post(format!("{}{path}", self.base)))
            .json(&body)
            .send()
            .await?
            .json()
            .await?)
    }
}

/// Fixture handler of [`MockTron`]: `(path, body)` → response.
pub type TronHandler = Box<dyn Fn(&str, Option<&Value>) -> anyhow::Result<Value> + Send + Sync>;

pub struct MockTron {
    pub handler: TronHandler,
}

#[async_trait]
impl TronApi for MockTron {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        (self.handler)(path, None)
    }

    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        (self.handler)(path, Some(&body))
    }
}

// ---------------------------------------------------------------- blocks

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowBlock {
    pub number: u64,
    pub id: [u8; 32],
    pub timestamp_ms: u64,
}

pub fn parse_block(v: &Value) -> anyhow::Result<NowBlock> {
    let raw = &v["block_header"]["raw_data"];
    Ok(NowBlock {
        number: raw["number"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("block number"))?,
        id: json_hex(&v["blockID"])
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| anyhow::anyhow!("blockID"))?,
        timestamp_ms: raw["timestamp"].as_u64().unwrap_or(0),
    })
}

pub async fn now_block(api: &dyn TronApi) -> anyhow::Result<NowBlock> {
    parse_block(&api.get("/wallet/getnowblock").await?)
}

/// Hex (`41…`) → base58 or pass a base58 address through.
pub fn to_base58(addr: &str) -> anyhow::Result<String> {
    if addr.starts_with('T') && addr.len() == 34 {
        return Ok(addr.to_string());
    }
    let bytes = hex::decode(addr.strip_prefix("0x").unwrap_or(addr))?;
    if bytes.len() != 21 || bytes[0] != 0x41 {
        anyhow::bail!("tron hex address must be 21 bytes with 0x41 prefix");
    }
    Ok(tron_address_from_account(
        bytes[1..].try_into().expect("20 bytes"),
    ))
}

// ---------------------------------------------------------------- deposits

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TronTransfer {
    pub txid: [u8; 32],
    pub txid_hex: String,
    pub to: String,
    pub amount: u128,
    pub timestamp_ms: u64,
    /// Token contract (base58) for TRC-20, `None` for TRX.
    pub token: Option<String>,
    pub block_number: Option<u64>,
}

/// `GET /v1/accounts/{addr}/transactions/trc20?only_to=true…`
pub fn parse_trc20_transfers(v: &Value) -> anyhow::Result<Vec<TronTransfer>> {
    let mut out = Vec::new();
    for t in v["data"].as_array().cloned().unwrap_or_default() {
        if t.get("type")
            .and_then(Value::as_str)
            .is_some_and(|ty| ty != "Transfer")
        {
            continue;
        }
        let txid_hex = t["transaction_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("transaction_id"))?
            .to_string();
        out.push(TronTransfer {
            txid: hex::decode(&txid_hex)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("txid length"))?,
            txid_hex,
            to: t["to"].as_str().unwrap_or_default().to_string(),
            amount: json_quantity(&t["value"]).ok_or_else(|| anyhow::anyhow!("value"))?,
            timestamp_ms: t["block_timestamp"].as_u64().unwrap_or(0),
            token: t["token_info"]["address"].as_str().map(str::to_string),
            block_number: None,
        });
    }
    Ok(out)
}

/// `GET /v1/accounts/{addr}/transactions?only_to=true…` → successful
/// `TransferContract`s.
pub fn parse_trx_transfers(v: &Value) -> anyhow::Result<Vec<TronTransfer>> {
    let mut out = Vec::new();
    for t in v["data"].as_array().cloned().unwrap_or_default() {
        let ok = t["ret"]
            .as_array()
            .and_then(|r| r.first())
            .and_then(|r| r["contractRet"].as_str())
            == Some("SUCCESS");
        let Some(c) = t["raw_data"]["contract"].as_array().and_then(|c| c.first()) else {
            continue;
        };
        if !ok || c["type"].as_str() != Some("TransferContract") {
            continue;
        }
        let value = &c["parameter"]["value"];
        let txid_hex = t["txID"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("txID"))?
            .to_string();
        out.push(TronTransfer {
            txid: hex::decode(&txid_hex)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("txid length"))?,
            txid_hex,
            to: to_base58(value["to_address"].as_str().unwrap_or_default())?,
            amount: json_quantity(&value["amount"]).ok_or_else(|| anyhow::anyhow!("amount"))?,
            timestamp_ms: t["block_timestamp"].as_u64().unwrap_or(0),
            token: None,
            block_number: t["blockNumber"].as_u64(),
        });
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxInfo {
    pub block_number: u64,
    pub success: bool,
    pub fee_sun: u64,
    /// (contract address hex without 0x41, topics, data) per log.
    pub logs: Vec<TronLog>,
}

/// (contract address without the 0x41 prefix, topics, data).
pub type TronLog = (Vec<u8>, Vec<Vec<u8>>, Vec<u8>);

/// `POST /wallet/gettransactioninfobyid` → `None` while unconfirmed.
pub fn parse_tx_info(v: &Value) -> Option<TxInfo> {
    let block_number = v["blockNumber"].as_u64()?;
    let result = v["receipt"]["result"].as_str();
    let success = result.is_none_or(|r| r == "SUCCESS");
    let logs = v["log"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| {
                    let addr = json_hex(&l["address"]).unwrap_or_default();
                    let topics = l["topics"]
                        .as_array()
                        .map(|t| t.iter().filter_map(json_hex).collect())
                        .unwrap_or_default();
                    (addr, topics, json_hex(&l["data"]).unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default();
    Some(TxInfo {
        block_number,
        success,
        fee_sun: v["fee"].as_u64().unwrap_or(0),
        logs,
    })
}

pub async fn tx_info(api: &dyn TronApi, txid_hex: &str) -> anyhow::Result<Option<TxInfo>> {
    Ok(parse_tx_info(
        &api.post(
            "/wallet/gettransactioninfobyid",
            json!({ "value": txid_hex }),
        )
        .await?,
    ))
}

/// Log position of the TRC-20 `Transfer(token → to, amount)` inside a tx.
pub fn transfer_log_position(info: &TxInfo, token: &str, to: &str, amount: u128) -> Option<u32> {
    let token = tron_decode(token).ok()?;
    let to = tron_decode(to).ok()?;
    let topic = keel_chains::eth::transfer_topic();
    info.logs
        .iter()
        .position(|(addr, topics, data)| {
            addr.as_slice() == &token[1..]
                && topics.len() == 3
                && topics[0] == topic
                && topics[2].len() == 32
                && topics[2][12..] == to[1..]
                && data.len() == 32
                && data[..16].iter().all(|b| *b == 0)
                && u128::from_be_bytes(data[16..].try_into().expect("16 bytes")) == amount
        })
        .map(|p| p as u32)
}

/// Transfers to every book address since `min_timestamp_ms` (TRC-20 per
/// configured token, then TRX).
pub async fn scan_deposits(
    api: &dyn TronApi,
    book: &AddressBook,
    tokens: &[(String, String)],
    min_timestamp_ms: u64,
) -> anyhow::Result<Vec<TronTransfer>> {
    let mut out = Vec::new();
    for (_, addr) in book.addresses() {
        for (_, contract) in tokens {
            let path = format!("/v1/accounts/{addr}/transactions/trc20?only_to=true&only_confirmed=true&limit=200&contract_address={contract}&min_timestamp={min_timestamp_ms}");
            out.extend(parse_trc20_transfers(&api.get(&path).await?)?);
        }
        let path = format!("/v1/accounts/{addr}/transactions?only_to=true&only_confirmed=true&limit=200&min_timestamp={min_timestamp_ms}");
        out.extend(parse_trx_transfers(&api.get(&path).await?)?);
    }
    Ok(out)
}

pub fn observation(
    t: &TronTransfer,
    asset: Asset,
    deposit_index: u64,
    index: u32,
    block_number: u64,
    tip: u64,
) -> DepositObservation {
    DepositObservation {
        chain: Chain::Tron,
        asset,
        tx_hash: t.txid,
        index,
        deposit_index,
        amount: t.amount,
        external_height: block_number,
        tip_height: tip,
        proof: Proof::None,
    }
}

// ---------------------------------------------------------------- outbound

pub async fn broadcast(api: &dyn TronApi, tx: Value) -> anyhow::Result<String> {
    let v = api.post("/wallet/broadcasttransaction", tx).await?;
    if v["result"].as_bool() != Some(true) {
        anyhow::bail!("broadcast failed: {v}");
    }
    v["txid"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("broadcast without txid"))
}

/// Find an outbound already paid from `from` (base58): TRC-20 or TRX.
pub async fn find_payment(
    api: &dyn TronApi,
    token: Option<&str>,
    from: &str,
    to: &str,
    amount: u128,
    min_timestamp_ms: u64,
) -> anyhow::Result<Option<String>> {
    let list = match token {
        Some(t) => parse_trc20_transfers(
            &api.get(&format!("/v1/accounts/{from}/transactions/trc20?only_from=true&limit=200&contract_address={t}&min_timestamp={min_timestamp_ms}")).await?,
        )?,
        None => parse_trx_transfers(&api.get(&format!("/v1/accounts/{from}/transactions?only_from=true&limit=200&min_timestamp={min_timestamp_ms}")).await?)?,
    };
    Ok(list
        .into_iter()
        .find(|t| t.to == to && t.amount == amount)
        .map(|t| t.txid_hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trc20_trx_lists_and_tx_info() {
        let trc20: Value = serde_json::from_str(
            r#"{"data":[{"transaction_id":"1f2e3d4c5b6a79887766554433221100ffeeddccbbaa99887766554433221100","token_info":{"symbol":"USDT","address":"TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t","decimals":6,"name":"Tether USD"},"block_timestamp":1700000000000,"from":"TLsV52sRDL79HXGGm9yzwKibb6BeruhUzy","to":"TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE","type":"Transfer","value":"12500000"}],"success":true,"meta":{"at":1,"page_size":1}}"#,
        )
        .unwrap();
        let t = parse_trc20_transfers(&trc20).unwrap();
        assert_eq!(t[0].amount, 12_500_000);
        assert_eq!(
            t[0].token.as_deref(),
            Some("TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t")
        );
        assert_eq!(t[0].to, "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE");

        let trx: Value = serde_json::from_str(
            r#"{"data":[{"ret":[{"contractRet":"SUCCESS","fee":1100000}],"txID":"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff","blockNumber":55000000,"block_timestamp":1700000001000,
              "raw_data":{"contract":[{"parameter":{"value":{"amount":5000000,"owner_address":"41a614f803b6fd780986a42c78ec9c7f77e6ded13c","to_address":"41a614f803b6fd780986a42c78ec9c7f77e6ded13c"},"type_url":"type.googleapis.com/protocol.TransferContract"},"type":"TransferContract"}]}},
              {"ret":[{"contractRet":"REVERT"}],"txID":"ff","raw_data":{"contract":[{"type":"TransferContract","parameter":{"value":{"amount":1,"to_address":"41a614f803b6fd780986a42c78ec9c7f77e6ded13c"}}}]}}]}"#,
        )
        .unwrap();
        let t = parse_trx_transfers(&trx).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].amount, 5_000_000);
        assert_eq!(t[0].to, "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t");
        assert_eq!(t[0].block_number, Some(55_000_000));

        let mut data = [0u8; 32];
        data[28..].copy_from_slice(&12_500_000u32.to_be_bytes());
        let to = tron_decode("TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE").unwrap();
        let mut to_topic = [0u8; 32];
        to_topic[12..].copy_from_slice(&to[1..]);
        let info: Value = json!({
            "id": "1f2e", "fee": 345000, "blockNumber": 55000010, "blockTimeStamp": 1700000000000u64, "receipt": { "result": "SUCCESS", "energy_usage_total": 13000 },
            "log": [
                { "address": "a614f803b6fd780986a42c78ec9c7f77e6ded13c", "topics": [hex::encode(keel_chains::eth::transfer_topic()), "00".repeat(32), hex::encode(to_topic)], "data": hex::encode(data) }
            ]
        });
        let info = parse_tx_info(&info).unwrap();
        assert_eq!(info.block_number, 55_000_010);
        assert!(info.success);
        assert_eq!(info.fee_sun, 345_000);
        assert_eq!(
            transfer_log_position(
                &info,
                "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
                "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE",
                12_500_000
            ),
            Some(0)
        );
        assert_eq!(
            transfer_log_position(
                &info,
                "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t",
                "TQn9Y2khEsLJW1ChVWFMSMeRDow5KcbLSE",
                1
            ),
            None
        );
        assert!(parse_tx_info(&json!({})).is_none());

        let nb: Value = json!({ "blockID": "0000000003473bf0" .to_string() + &"aa".repeat(24), "block_header": { "raw_data": { "number": 55000048, "timestamp": 1700000003000u64 } } });
        let b = parse_block(&nb).unwrap();
        assert_eq!(b.number, 55_000_048);
        assert_eq!(&b.id[8..16], &[0xaa; 8]);
        let (rb, _) = keel_chains::tron::ref_block(b.number, &b.id);
        assert_eq!(rb, [0x3b, 0xf0]);
    }
}
