//! Bitcoin Core: watch-only descriptor wallet for the deposit addresses,
//! SPV proof material (`gettxoutproof` + headers to the tip) in exactly
//! the format `keel-lc-btc` verifies, fee estimates and broadcast.

use super::{json_amount, AddressBook};
use crate::config::BitcoinConfig;
use async_trait::async_trait;
use bitcoin::{consensus, hashes::Hash as _, MerkleBlock, Txid};
use keel_actions::{Chain, DepositObservation, Proof};
use keel_chains::btc::Utxo;
use keel_types::Asset;
use serde_json::{json, Value};
use std::str::FromStr as _;

#[async_trait]
pub trait BitcoinRpc: Send + Sync {
    /// Node-level JSON-RPC call.
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value>;
    /// Wallet-level call (`/wallet/<name>`).
    async fn wallet_call(&self, method: &str, params: Value) -> anyhow::Result<Value>;
}

pub struct HttpBitcoin {
    base: String,
    wallet: String,
    user: String,
    password: String,
    client: reqwest::Client,
}

impl HttpBitcoin {
    pub fn new(cfg: &BitcoinConfig) -> Self {
        Self {
            base: cfg.rpc_url.trim_end_matches('/').to_string(),
            wallet: cfg.wallet.clone(),
            user: cfg.rpc_user.clone(),
            password: cfg.rpc_password.clone(),
            client: reqwest::Client::new(),
        }
    }

    async fn rpc(&self, url: String, method: &str, params: Value) -> anyhow::Result<Value> {
        let body =
            json!({ "jsonrpc": "1.0", "id": "keel-observer", "method": method, "params": params });
        let resp = self
            .client
            .post(url)
            .basic_auth(&self.user, Some(&self.password))
            .json(&body)
            .send()
            .await?;
        let v: Value = resp.json().await?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            anyhow::bail!("bitcoin {method}: {err}");
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[async_trait]
impl BitcoinRpc for HttpBitcoin {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.rpc(self.base.clone(), method, params).await
    }

    async fn wallet_call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        self.rpc(
            format!("{}/wallet/{}", self.base, self.wallet),
            method,
            params,
        )
        .await
    }
}

/// Fixture-driven mock: a handler that answers `(method, params)`.
pub struct MockBitcoin {
    #[allow(clippy::type_complexity)]
    pub handler: Box<dyn Fn(&str, &Value) -> anyhow::Result<Value> + Send + Sync>,
}

#[async_trait]
impl BitcoinRpc for MockBitcoin {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        (self.handler)(method, &params)
    }

    async fn wallet_call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        (self.handler)(method, &params)
    }
}

// ---------------------------------------------------------------- wallet

/// Create/load the watch-only wallet and import every address of the book
/// not yet imported (`imported_upto` is the exclusive cursor).
pub async fn ensure_wallet(
    rpc: &dyn BitcoinRpc,
    wallet: &str,
    book: &AddressBook,
    imported_upto: u64,
    rescan: bool,
) -> anyhow::Result<u64> {
    let loaded = rpc.call("listwallets", json!([])).await?;
    let is_loaded = loaded
        .as_array()
        .is_some_and(|a| a.iter().any(|w| w.as_str() == Some(wallet)));
    if !is_loaded {
        match rpc.call("loadwallet", json!([wallet])).await {
            Ok(_) => {}
            Err(_) => {
                // disable_private_keys=true, blank=true, descriptors=true, load_on_startup=true
                rpc.call(
                    "createwallet",
                    json!([wallet, true, true, "", false, true, true]),
                )
                .await?;
            }
        }
    }
    let mut requests = Vec::new();
    let mut upto = imported_upto;
    for (index, addr) in book.addresses() {
        if index < imported_upto {
            continue;
        }
        let bare = format!("addr({addr})");
        let info = rpc.call("getdescriptorinfo", json!([bare])).await?;
        let desc = info["descriptor"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("getdescriptorinfo.descriptor"))?
            .to_string();
        requests.push(json!({ "desc": desc, "timestamp": if rescan { json!(0) } else { json!("now") }, "label": format!("keel:{index}") }));
        upto = index + 1;
    }
    if !requests.is_empty() {
        let res = rpc
            .wallet_call("importdescriptors", json!([requests]))
            .await?;
        if let Some(bad) = res
            .as_array()
            .and_then(|a| a.iter().find(|r| r["success"].as_bool() == Some(false)))
        {
            anyhow::bail!("importdescriptors: {bad}");
        }
    }
    Ok(upto)
}

// ---------------------------------------------------------------- deposits

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletUtxo {
    /// Internal byte order.
    pub txid: [u8; 32],
    pub txid_hex: String,
    pub vout: u32,
    pub address: String,
    pub value_sat: u64,
    pub confirmations: u64,
}

/// Parse `listunspent` output.
pub fn parse_listunspent(v: &Value) -> anyhow::Result<Vec<WalletUtxo>> {
    let mut out = Vec::new();
    for u in v.as_array().cloned().unwrap_or_default() {
        let txid_hex = u["txid"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("listunspent.txid"))?
            .to_string();
        let txid = Txid::from_str(&txid_hex)?.to_byte_array();
        out.push(WalletUtxo {
            txid,
            txid_hex,
            vout: u["vout"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("listunspent.vout"))? as u32,
            address: u["address"].as_str().unwrap_or_default().to_string(),
            value_sat: json_amount(&u["amount"], 8)
                .ok_or_else(|| anyhow::anyhow!("listunspent.amount"))?
                as u64,
            confirmations: u["confirmations"].as_u64().unwrap_or(0),
        });
    }
    Ok(out)
}

/// All watched UTXOs with at least `minconf` confirmations.
pub async fn list_unspent(rpc: &dyn BitcoinRpc, minconf: u64) -> anyhow::Result<Vec<WalletUtxo>> {
    parse_listunspent(
        &rpc.wallet_call("listunspent", json!([minconf, 9_999_999]))
            .await?,
    )
}

pub async fn block_count(rpc: &dyn BitcoinRpc) -> anyhow::Result<u64> {
    rpc.call("getblockcount", json!([]))
        .await?
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("getblockcount"))
}

/// Block hash and height of the block containing a wallet transaction.
pub async fn tx_block(
    rpc: &dyn BitcoinRpc,
    txid_hex: &str,
) -> anyhow::Result<Option<(String, u64)>> {
    let tx = rpc
        .wallet_call("gettransaction", json!([txid_hex, true]))
        .await?;
    let Some(hash) = tx["blockhash"].as_str() else {
        return Ok(None);
    };
    let header = rpc.call("getblockheader", json!([hash, true])).await?;
    let height = header["height"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("getblockheader.height"))?;
    Ok(Some((hash.to_string(), height)))
}

/// Raw 80-byte headers for `from..=to`.
pub async fn headers(rpc: &dyn BitcoinRpc, from: u64, to: u64) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut out = Vec::with_capacity((to.saturating_sub(from) + 1) as usize);
    for h in from..=to {
        let hash = rpc.call("getblockhash", json!([h])).await?;
        let hash = hash
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("getblockhash"))?;
        let raw = rpc.call("getblockheader", json!([hash, false])).await?;
        let bytes = hex::decode(
            raw.as_str()
                .ok_or_else(|| anyhow::anyhow!("getblockheader raw"))?,
        )?;
        if bytes.len() != 80 {
            anyhow::bail!("header at {h} is {} bytes", bytes.len());
        }
        out.push(bytes);
    }
    Ok(out)
}

/// `gettxoutproof` returns a `MerkleBlock` (header + partial merkle tree);
/// the VM wants the partial merkle tree alone plus the tx position.
pub fn split_txoutproof(
    proof_hex: &str,
    txid: &[u8; 32],
) -> anyhow::Result<(Vec<u8>, u32, [u8; 32])> {
    let bytes = hex::decode(proof_hex)?;
    let mb: MerkleBlock = consensus::deserialize(&bytes)?;
    let mut matches = Vec::new();
    let mut indexes = Vec::new();
    mb.txn
        .extract_matches(&mut matches, &mut indexes)
        .map_err(|e| anyhow::anyhow!("partial merkle tree: {e:?}"))?;
    let want = Txid::from_byte_array(*txid);
    let pos = matches
        .iter()
        .position(|t| *t == want)
        .ok_or_else(|| anyhow::anyhow!("txid not in proof"))?;
    Ok((
        consensus::serialize(&mb.txn),
        indexes[pos],
        mb.header.merkle_root.to_byte_array(),
    ))
}

/// Build the observation for one UTXO on a deposit address.
///
/// `index` is the transaction's position in its block (the VM requires
/// `index == proof.tx_index`), so a transaction paying two vault outputs
/// yields two observations that the chain treats as one key; see
/// README.md.
pub async fn build_observation(
    rpc: &dyn BitcoinRpc,
    utxo: &WalletUtxo,
    deposit_index: u64,
    max_headers: u64,
) -> anyhow::Result<Option<DepositObservation>> {
    let Some((block_hash, height)) = tx_block(rpc, &utxo.txid_hex).await? else {
        return Ok(None);
    };
    let tip = block_count(rpc).await?;
    let last = tip.min(height + max_headers.max(1) - 1);
    let headers = headers(rpc, height, last).await?;
    let proof_hex = rpc
        .call("gettxoutproof", json!([[utxo.txid_hex], block_hash]))
        .await?;
    let proof_hex = proof_hex
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("gettxoutproof"))?;
    let (merkle_proof, tx_index, _root) = split_txoutproof(proof_hex, &utxo.txid)?;
    Ok(Some(DepositObservation {
        chain: Chain::Bitcoin,
        asset: Asset::vault("BTC", "BTC"),
        tx_hash: utxo.txid,
        // Output index: the credit key on chain is (txid, vout).
        index: utxo.vout,
        deposit_index,
        amount: utxo.value_sat as u128,
        external_height: height,
        tip_height: last,
        proof: Proof::Bitcoin {
            headers,
            merkle_proof,
            tx_index,
        },
    }))
}

// ---------------------------------------------------------------- fees / broadcast

/// `estimatesmartfee` feerate is BTC/kvB; the chain wants sat/vB.
pub fn parse_estimatesmartfee(v: &Value) -> Option<u64> {
    let per_kvb = json_amount(v.get("feerate")?, 8)?;
    Some((per_kvb / 1000).max(1) as u64)
}

pub async fn fee_rate(rpc: &dyn BitcoinRpc, target: u32, fallback: u64) -> anyhow::Result<u64> {
    let v = rpc.call("estimatesmartfee", json!([target])).await?;
    Ok(parse_estimatesmartfee(&v).unwrap_or(fallback))
}

pub async fn send_raw(rpc: &dyn BitcoinRpc, raw: &[u8]) -> anyhow::Result<String> {
    let v = rpc
        .call("sendrawtransaction", json!([hex::encode(raw)]))
        .await?;
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("sendrawtransaction"))
}

/// Wallet UTXOs as `keel-chains` inputs (only addresses in the book).
pub fn vault_utxos(utxos: &[WalletUtxo], book: &AddressBook) -> anyhow::Result<Vec<Utxo>> {
    let mut out = Vec::new();
    for u in utxos {
        let Some(key_index) = book.index_of(&u.address) else {
            continue;
        };
        out.push(Utxo {
            txid: u.txid,
            vout: u.vout,
            value: u.value_sat,
            key_index,
            pubkey: book.pubkey(key_index)?,
        });
    }
    Ok(out)
}

/// Confirmation status of a broadcast transaction: (height, confirmations).
/// `None` while unconfirmed or when the wallet does not know the
/// transaction (a broadcast that never reached the node).
pub async fn tx_confirmations(
    rpc: &dyn BitcoinRpc,
    txid_hex: &str,
) -> anyhow::Result<Option<(u64, u64)>> {
    let tx = match rpc
        .wallet_call("gettransaction", json!([txid_hex, true]))
        .await
    {
        Ok(v) => v,
        Err(e) if e.to_string().contains("non-wallet transaction") => return Ok(None),
        Err(e) => return Err(e),
    };
    let conf = tx["confirmations"].as_i64().unwrap_or(0);
    if conf <= 0 {
        return Ok(None);
    }
    match tx_block(rpc, txid_hex).await? {
        Some((_, height)) => Ok(Some((height, conf as u64))),
        None => Ok(None),
    }
}

/// Fee the wallet reports for one of its own sends, in sats (`fee` of
/// `gettransaction` is negative BTC); `None` when the wallet cannot tell
/// (not every input is watched).
pub async fn tx_fee_sats(rpc: &dyn BitcoinRpc, txid_hex: &str) -> anyhow::Result<Option<u64>> {
    let tx = rpc
        .wallet_call("gettransaction", json!([txid_hex, true]))
        .await?;
    let Some(fee) = tx.get("fee") else {
        return Ok(None);
    };
    let text = fee.to_string();
    Ok(json_amount(&json!(text.trim_start_matches('-')), 8).map(|v| v as u64))
}

/// Whether the node's mempool holds `txid`.
pub async fn in_mempool(rpc: &dyn BitcoinRpc, txid_hex: &str) -> bool {
    rpc.call("getmempoolentry", json!([txid_hex])).await.is_ok()
}

/// Look for an outbound the vault already paid: a wallet "send" to `to`
/// of `amount_sat` (`listsinceblock` over all watched outputs).
pub async fn find_payment(
    rpc: &dyn BitcoinRpc,
    to: &str,
    amount_sat: u64,
) -> anyhow::Result<Option<String>> {
    let v = rpc
        .wallet_call("listsinceblock", json!([Value::Null, 1, true]))
        .await?;
    for t in v["transactions"].as_array().cloned().unwrap_or_default() {
        if t["category"].as_str() != Some("send") || t["address"].as_str() != Some(to) {
            continue;
        }
        let amt = t["amount"]
            .as_f64()
            .map(|f| f.abs())
            .and_then(|f| json_amount(&json!(f), 8));
        if amt == Some(amount_sat as u128) {
            if let Some(txid) = t["txid"].as_str() {
                return Ok(Some(txid.to_string()));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! A regtest-like chain mined in-process: block 1 holds the deposit.
    use bitcoin::{
        block::{Header, Version},
        blockdata::constants::genesis_block,
        hashes::{sha256d, Hash as _},
        merkle_tree::PartialMerkleTree,
        Network, TxMerkleNode, Txid,
    };

    pub fn txid(n: u8) -> Txid {
        Txid::from_byte_array(sha256d::Hash::hash(&[n]).to_byte_array())
    }

    fn mine(prev: &Header, merkle_root: TxMerkleNode, time: u32) -> Header {
        let mut h = Header {
            version: Version::TWO,
            prev_blockhash: prev.block_hash(),
            merkle_root,
            time,
            bits: prev.bits,
            nonce: 0,
        };
        while h.validate_pow(h.target()).is_err() {
            h.nonce += 1;
        }
        h
    }

    /// (headers[0..=n], merkle block hex for `deposit_txid` in block 1)
    pub fn chain(n: usize, txids: &[Txid], matched: usize) -> (Vec<Header>, String) {
        let genesis = genesis_block(Network::Regtest).header;
        let matches: Vec<bool> = (0..txids.len()).map(|i| i == matched).collect();
        let pmt = PartialMerkleTree::from_txids(txids, &matches);
        let mut m = Vec::new();
        let mut idx = Vec::new();
        let root = pmt.extract_matches(&mut m, &mut idx).unwrap();
        let mut headers = vec![genesis];
        for i in 0..n {
            let prev = *headers.last().unwrap();
            let mr = if i == 0 {
                root
            } else {
                TxMerkleNode::from_byte_array(sha256d::Hash::hash(&[i as u8]).to_byte_array())
            };
            headers.push(mine(&prev, mr, prev.time + 600));
        }
        let mb = bitcoin::MerkleBlock {
            header: headers[1],
            txn: pmt,
        };
        (headers, hex::encode(bitcoin::consensus::serialize(&mb)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listunspent_and_fee_estimates() {
        let v: Value = serde_json::from_str(
            r#"[{"txid":"4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b","vout":1,"address":"bcrt1qxyz","label":"keel:3","scriptPubKey":"0014aa","amount":0.00150000,"confirmations":3,"spendable":false,"solvable":true,"desc":"addr(bcrt1qxyz)#abc","safe":true}]"#,
        )
        .unwrap();
        let u = parse_listunspent(&v).unwrap();
        assert_eq!(u[0].value_sat, 150_000);
        assert_eq!(u[0].vout, 1);
        assert_eq!(u[0].confirmations, 3);
        assert_eq!(
            u[0].txid,
            Txid::from_str("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
                .unwrap()
                .to_byte_array()
        );
        assert_eq!(
            parse_estimatesmartfee(&serde_json::json!({ "feerate": 0.00012345, "blocks": 2 })),
            Some(12)
        );
        assert_eq!(
            parse_estimatesmartfee(&serde_json::json!({ "feerate": 0.00001, "blocks": 2 })),
            Some(1)
        );
        assert_eq!(
            parse_estimatesmartfee(
                &serde_json::json!({ "errors": ["Insufficient data"], "blocks": 2 })
            ),
            None
        );
    }

    #[tokio::test]
    async fn builds_an_observation_that_stt_lc_btc_verifies() {
        let txids = vec![fixtures::txid(1), fixtures::txid(2), fixtures::txid(3)];
        let (headers, proof_hex) = fixtures::chain(5, &txids, 1);
        let deposit = txids[1];
        let tip = headers.len() as u64 - 1;
        let hs = headers.clone();
        let mock = MockBitcoin {
            handler: Box::new(move |method, params| {
                Ok(match method {
                    "getblockcount" => json!(tip),
                    "gettransaction" => {
                        json!({ "txid": params[0], "blockhash": hs[1].block_hash().to_string(), "confirmations": tip })
                    }
                    "getblockheader" => {
                        let hash = params[0].as_str().unwrap();
                        let (h, header) = hs
                            .iter()
                            .enumerate()
                            .find(|(_, h)| h.block_hash().to_string() == hash)
                            .unwrap();
                        if params[1].as_bool() == Some(true) {
                            json!({ "height": h, "hash": hash })
                        } else {
                            json!(hex::encode(bitcoin::consensus::serialize(header)))
                        }
                    }
                    "getblockhash" => json!(hs[params[0].as_u64().unwrap() as usize]
                        .block_hash()
                        .to_string()),
                    "gettxoutproof" => json!(proof_hex),
                    m => anyhow::bail!("unexpected {m}"),
                })
            }),
        };
        let utxo = WalletUtxo {
            txid: deposit.to_byte_array(),
            txid_hex: deposit.to_string(),
            vout: 0,
            address: "bcrt1q".into(),
            value_sat: 70_000,
            confirmations: tip,
        };
        let obs = build_observation(&mock, &utxo, 7, 144)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(obs.index, 0, "index is the vout");
        assert_eq!(obs.external_height, 1);
        assert_eq!(obs.tip_height, tip);
        assert_eq!(obs.amount, 70_000);
        let Proof::Bitcoin {
            headers: hdrs,
            merkle_proof,
            tx_index,
        } = &obs.proof
        else {
            panic!("btc proof")
        };
        assert_eq!(
            *tx_index, 1,
            "merkle position of the transaction in its block"
        );
        assert_eq!(hdrs.len(), tip as usize);
        let v = keel_lc_btc::verify_deposit(
            hdrs,
            merkle_proof,
            *tx_index,
            obs.tx_hash,
            4,
            keel_lc_btc::Network::Regtest,
        )
        .unwrap();
        assert_eq!(v.depth, 4);
        assert_eq!(v.block_hash, headers[1].block_hash().to_byte_array());
        // A header cap shortens the chain and the reported tip together.
        let capped = build_observation(&mock, &utxo, 7, 3)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(capped.tip_height, 3);
        let Proof::Bitcoin { headers: hdrs, .. } = &capped.proof else {
            panic!()
        };
        assert_eq!(hdrs.len(), 3);
        // The VM's header chain accepts it too.
        let mut chain = keel_lc_btc::HeaderChain::from_checkpoint(
            keel_lc_btc::Network::Regtest,
            0,
            &keel_lc_btc::encode_header(&headers[0]),
            headers[0].time,
        )
        .unwrap();
        let Proof::Bitcoin {
            headers: hdrs,
            merkle_proof,
            tx_index,
        } = &obs.proof
        else {
            panic!()
        };
        assert!(chain
            .verify_deposit(hdrs, merkle_proof, *tx_index, obs.tx_hash, 2)
            .is_ok());
    }
}
