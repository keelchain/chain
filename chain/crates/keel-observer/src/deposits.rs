//! Deposit scanning: one pass per chain turns new transfers to vault
//! addresses into `ObserveDeposit` actions, once each (idempotency keys
//! in the state file), only when deep enough for the chain's parameter.

use crate::{
    chains::{
        btc::{self, BitcoinRpc},
        eth::{self, BeaconApi, EthRpc, FinalityCache},
        tron::{self, TronApi},
        AddressBook,
    },
    config::{BitcoinConfig, EthereumConfig, TronConfig},
    rpc::{ChainParams, Submitter},
    state::StateFile,
};
use keel_actions::{Action, Chain, DepositObservation, Proof};
use keel_types::Asset;
use std::collections::HashMap;

pub struct DepositContext<'a> {
    pub submitter: &'a Submitter,
    pub state: &'a StateFile,
    pub params: &'a ChainParams,
}

impl DepositContext<'_> {
    async fn submit(&self, key: &str, obs: DepositObservation) -> anyhow::Result<bool> {
        let res = self
            .submitter
            .submit(Some(key), Action::ObserveDeposit(obs))
            .await?;
        if res.admitted {
            tracing::info!(key, tx_id = ?res.tx_id, "deposit observation submitted");
        }
        Ok(res.admitted)
    }
}

/// One Bitcoin pass: every watched UTXO deep enough and not yet submitted.
pub async fn scan_bitcoin(
    ctx: &DepositContext<'_>,
    rpc: &dyn BitcoinRpc,
    book: &AddressBook,
    cfg: &BitcoinConfig,
) -> anyhow::Result<usize> {
    let required = ctx.params.confirmations(Chain::Bitcoin) as u64;
    let utxos = btc::list_unspent(rpc, 1).await?;
    let mut submitted = 0;
    for u in utxos {
        let Some(index) = book.index_of(&u.address) else {
            continue;
        };
        if index == 0 || book.owner(index).is_none() {
            continue; // hot/change address or an index the chain has not assigned
        }
        let key = format!("deposit:BTC:{}:{}", u.txid_hex, u.vout);
        if ctx.state.is_submitted(&key) || u.confirmations.saturating_sub(1) < required {
            continue;
        }
        let Some(obs) = btc::build_observation(rpc, &u, index, cfg.max_proof_headers).await? else {
            continue;
        };
        if obs.tip_height.saturating_sub(obs.external_height) < required {
            continue;
        }
        if ctx.submit(&key, obs).await? {
            submitted += 1;
        }
    }
    Ok(submitted)
}

/// State kept across Ethereum passes.
pub struct EthScanner {
    pub cache: FinalityCache,
    pub native_balances: HashMap<u64, u128>,
}

impl Default for EthScanner {
    fn default() -> Self {
        Self {
            cache: FinalityCache::new(128),
            native_balances: HashMap::new(),
        }
    }
}

/// One Ethereum pass. With a beacon node the window only advances over
/// finalized blocks whose finality update was captured, so every ERC-20
/// deposit in a checkpoint block gets its proof; the rest are recorded
/// as `unprovable:` (see README.md). Without a beacon node nothing can be
/// submitted for Ethereum.
pub async fn scan_ethereum(
    ctx: &DepositContext<'_>,
    rpc: &dyn EthRpc,
    beacon: Option<&dyn BeaconApi>,
    scanner: &mut EthScanner,
    book: &AddressBook,
    cfg: &EthereumConfig,
) -> anyhow::Result<usize> {
    let required = ctx.params.confirmations(Chain::Ethereum) as u64;
    let tip = eth::block_number(rpc).await?;
    if let Some(b) = beacon {
        match eth::fetch_finality_update(b).await {
            Ok(u) => scanner.cache.insert(u),
            Err(e) => tracing::warn!(error = %e, "finality update unavailable"),
        }
    }
    watch_native(rpc, book, &mut scanner.native_balances).await;

    let upper = match (beacon, scanner.cache.finalized_block()) {
        (Some(_), Some(f)) => f.min(tip.saturating_sub(required)),
        (Some(_), None) => return Ok(0),
        (None, _) => tip.saturating_sub(required),
    };
    let from = match ctx.state.cursor("eth:last_block") {
        Some(c) => c + 1,
        None => upper.saturating_sub(cfg.log_window).max(1),
    };
    if from > upper {
        return Ok(0);
    }
    let to = upper.min(from + cfg.log_window - 1);
    let mut submitted = 0;
    for token in &cfg.tokens {
        let asset = Asset::vault("ETH", &token.symbol);
        for log in eth::scan_transfers(rpc, &token.contract, book, from, to).await? {
            let to_addr = keel_chains::address::eth_checksum(&log.to);
            let Some(index) = book.index_of(&to_addr) else {
                continue;
            };
            if index == 0 || book.owner(index).is_none() {
                continue;
            }
            let key = format!("deposit:ETH:{}:{}", log.tx_hash_hex, log.log_index);
            if ctx.state.is_submitted(&key) || ctx.state.is_submitted(&format!("unprovable:{key}"))
            {
                continue;
            }
            let Some(position) =
                eth::log_position_in_receipt(rpc, &log.tx_hash_hex, log.log_index).await?
            else {
                tracing::warn!(tx = %log.tx_hash_hex, "log not found in its receipt");
                continue;
            };
            let proof = match (beacon, scanner.cache.get(log.block_number)) {
                (Some(b), Some(update)) => {
                    let committee = eth::fetch_committee(
                        b,
                        eth::period_of_slot(update.signature_slot),
                        &update.finalized.hash_tree_root(),
                    )
                    .await?;
                    let receipts = eth::block_receipts(rpc, log.block_number).await?;
                    Some(eth::build_proof(
                        update,
                        &committee,
                        &receipts,
                        log.tx_index,
                        position,
                    )?)
                }
                _ => None,
            };
            let obs = eth::observation(&log, asset.clone(), index, position, tip, proof.as_ref());
            if obs.proof == Proof::None {
                tracing::warn!(key, block = log.block_number, amount = log.amount, "ERC-20 deposit outside a finalized checkpoint block: no light-client proof, not submitted");
                ctx.state.mark_submitted(&format!("unprovable:{key}"), "")?;
                continue;
            }
            if ctx.submit(&key, obs).await? {
                submitted += 1;
            }
        }
    }
    ctx.state.set_cursor("eth:last_block", to)?;
    Ok(submitted)
}

/// Log native balance increases; the VM cannot credit them (no receipt
/// log to prove and no token contract for `ETH.ETH`).
async fn watch_native(rpc: &dyn EthRpc, book: &AddressBook, last: &mut HashMap<u64, u128>) {
    for (index, addr) in book.addresses() {
        let Ok(bal) = eth::balance(rpc, addr).await else {
            continue;
        };
        if let Some(prev) = last.get(&index) {
            if bal > *prev && index != 0 {
                tracing::warn!(
                    index,
                    addr,
                    delta = bal - prev,
                    "native ETH deposit detected; not provable, not submitted"
                );
            }
        }
        last.insert(index, bal);
    }
}

/// One Tron pass (attestation only).
pub async fn scan_tron(
    ctx: &DepositContext<'_>,
    api: &dyn TronApi,
    book: &AddressBook,
    cfg: &TronConfig,
) -> anyhow::Result<usize> {
    let required = ctx.params.confirmations(Chain::Tron) as u64;
    let now = tron::now_block(api).await?;
    let tokens: Vec<(String, String)> = cfg
        .tokens
        .iter()
        .map(|t| (t.symbol.clone(), t.contract.clone()))
        .collect();
    let min_ts = ctx
        .state
        .cursor("tron:min_timestamp")
        .unwrap_or(now.timestamp_ms.saturating_sub(24 * 3600 * 1000));
    let transfers = tron::scan_deposits(api, book, &tokens, min_ts).await?;
    let mut submitted = 0;
    let mut pending_ts: Option<u64> = None;
    let mut max_ts = min_ts;
    for t in transfers {
        max_ts = max_ts.max(t.timestamp_ms);
        let Some(index) = book.index_of(&t.to) else {
            continue;
        };
        if index == 0 || book.owner(index).is_none() {
            continue;
        }
        let (asset, symbol) = match &t.token {
            Some(contract) => match tokens.iter().find(|(_, c)| c == contract) {
                Some((s, _)) => (Asset::vault("TRON", s), s.clone()),
                None => continue,
            },
            None => (Asset::vault("TRON", "TRX"), "TRX".into()),
        };
        let key = format!("deposit:TRON:{}:{}", t.txid_hex, symbol);
        if ctx.state.is_submitted(&key) {
            continue;
        }
        let Some(info) = tron::tx_info(api, &t.txid_hex).await? else {
            pending_ts = Some(pending_ts.map_or(t.timestamp_ms, |p| p.min(t.timestamp_ms)));
            continue;
        };
        if !info.success {
            continue;
        }
        if now.number.saturating_sub(info.block_number) < required {
            pending_ts = Some(pending_ts.map_or(t.timestamp_ms, |p| p.min(t.timestamp_ms)));
            continue;
        }
        let log_index = match &t.token {
            Some(contract) => match tron::transfer_log_position(&info, contract, &t.to, t.amount) {
                Some(p) => p,
                None => {
                    tracing::warn!(tx = %t.txid_hex, "TRC-20 transfer log not found in transaction info");
                    continue;
                }
            },
            None => 0,
        };
        let obs = tron::observation(&t, asset, index, log_index, info.block_number, now.number);
        if ctx.submit(&key, obs).await? {
            submitted += 1;
        }
    }
    ctx.state
        .set_cursor("tron:min_timestamp", pending_ts.unwrap_or(max_ts))?;
    Ok(submitted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        chains::{btc::fixtures, btc::MockBitcoin, tron::MockTron},
        rpc::{MockSttRpc, VaultView},
    };
    use bitcoin::hashes::Hash as _;
    use keel_actions::CHAIN_ID_DEVNET;
    use keel_chains::Network;
    use keel_crypto::Keypair;
    use keel_types::Address;
    use serde_json::json;
    use std::{collections::BTreeMap, sync::Arc};

    fn vault(chain: Chain) -> VaultView {
        let signer = crate::tss::LocalSigner::from_seed(&[1u8; 32]).unwrap();
        VaultView {
            chain,
            epoch: 1,
            public_key: signer.public_key(),
            chain_code: signer.chain_code(),
            signers: vec![Keypair::from_seed(3).address()],
            threshold: 1,
            next_deposit_index: 3,
            owners: BTreeMap::from([(1, Address::tagged(1)), (2, Address::tagged(2))]),
        }
    }

    #[tokio::test]
    async fn bitcoin_pass_submits_once_and_only_when_deep() {
        let book = AddressBook::build(vault(Chain::Bitcoin), Network::Regtest).unwrap();
        let addr1 = book.address(1).unwrap().to_string();
        let txids = vec![fixtures::txid(1), fixtures::txid(2)];
        let (headers, proof_hex) = fixtures::chain(4, &txids, 0);
        let tip = headers.len() as u64 - 1;
        let deposit = txids[0];
        let hs = headers.clone();
        let mock = MockBitcoin {
            handler: Box::new(move |method, params| {
                Ok(match method {
                    "listunspent" => json!([
                        { "txid": deposit.to_string(), "vout": 0, "address": addr1, "amount": 0.5, "confirmations": tip },
                        { "txid": deposit.to_string(), "vout": 1, "address": "bcrt1qunknown", "amount": 0.1, "confirmations": tip },
                    ]),
                    "getblockcount" => json!(tip),
                    "gettransaction" => {
                        json!({ "blockhash": hs[1].block_hash().to_string(), "confirmations": tip })
                    }
                    "getblockheader" => {
                        let hash = params[0].as_str().unwrap();
                        let (h, header) = hs
                            .iter()
                            .enumerate()
                            .find(|(_, h)| h.block_hash().to_string() == hash)
                            .unwrap();
                        if params[1].as_bool() == Some(true) {
                            json!({ "height": h })
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
        let rpc = Arc::new(MockSttRpc {
            chain_id: CHAIN_ID_DEVNET,
            ..Default::default()
        });
        let state = Arc::new(StateFile::ephemeral());
        let submitter = Submitter::new(
            rpc.clone(),
            Keypair::from_seed(3),
            CHAIN_ID_DEVNET,
            state.clone(),
        );
        let cfg = BitcoinConfig {
            rpc_url: String::new(),
            rpc_user: String::new(),
            rpc_password: String::new(),
            network: Network::Regtest,
            wallet: "w".into(),
            max_proof_headers: 144,
            fee_target_blocks: 2,
            fallback_sat_per_vb: 1,
        };
        // Requires more depth than available: nothing submitted.
        let deep = ChainParams {
            confirmations_btc: 10,
            ..Default::default()
        };
        let ctx = DepositContext {
            submitter: &submitter,
            state: &state,
            params: &deep,
        };
        assert_eq!(scan_bitcoin(&ctx, &mock, &book, &cfg).await.unwrap(), 0);
        let params = ChainParams {
            confirmations_btc: 2,
            ..Default::default()
        };
        let ctx = DepositContext {
            submitter: &submitter,
            state: &state,
            params: &params,
        };
        assert_eq!(scan_bitcoin(&ctx, &mock, &book, &cfg).await.unwrap(), 1);
        assert_eq!(scan_bitcoin(&ctx, &mock, &book, &cfg).await.unwrap(), 0);
        let submitted = rpc.submitted.lock().unwrap();
        assert_eq!(submitted.len(), 1);
        let Action::ObserveDeposit(o) = &submitted[0].envelope.action else {
            panic!()
        };
        assert_eq!(o.deposit_index, 1);
        assert_eq!(o.amount, 50_000_000);
        assert_eq!(o.tx_hash, deposit.to_byte_array());
        assert!(matches!(o.proof, Proof::Bitcoin { .. }));
        assert!(state.is_submitted(&format!("deposit:BTC:{deposit}:0")));
    }

    #[tokio::test]
    async fn tron_pass_waits_for_depth_and_keeps_a_cursor() {
        let book = AddressBook::build(vault(Chain::Tron), Network::Mainnet).unwrap();
        let addr1 = book.address(1).unwrap().to_string();
        let a1 = addr1.clone();
        let mock = MockTron {
            handler: Box::new(move |path, body| {
                if path == "/wallet/getnowblock" {
                    return Ok(
                        json!({ "blockID": "00".repeat(32), "block_header": { "raw_data": { "number": 1000, "timestamp": 5_000_000u64 } } }),
                    );
                }
                if path.contains("/transactions/trc20") {
                    return Ok(json!({ "data": [] }));
                }
                if path.contains(&format!("/v1/accounts/{a1}/transactions?")) {
                    return Ok(json!({ "data": [
                        { "ret": [{ "contractRet": "SUCCESS" }], "txID": "aa".repeat(32), "blockNumber": 990, "block_timestamp": 4_000_000u64,
                          "raw_data": { "contract": [{ "type": "TransferContract", "parameter": { "value": { "amount": 7_000_000, "to_address": format!("41{}", hex::encode(&keel_chains::address::tron_decode(&a1).unwrap()[1..])) } } }] } },
                        { "ret": [{ "contractRet": "SUCCESS" }], "txID": "bb".repeat(32), "blockNumber": 998, "block_timestamp": 4_500_000u64,
                          "raw_data": { "contract": [{ "type": "TransferContract", "parameter": { "value": { "amount": 1, "to_address": format!("41{}", hex::encode(&keel_chains::address::tron_decode(&a1).unwrap()[1..])) } } }] } }
                    ]}));
                }
                if path.contains("/transactions?") {
                    return Ok(json!({ "data": [] }));
                }
                if path == "/wallet/gettransactioninfobyid" {
                    let id = body.unwrap()["value"].as_str().unwrap().to_string();
                    let block = if id.starts_with("aa") { 990 } else { 998 };
                    return Ok(
                        json!({ "id": id, "blockNumber": block, "fee": 100000, "receipt": { "result": "SUCCESS" } }),
                    );
                }
                anyhow::bail!("unexpected {path}")
            }),
        };
        let rpc = Arc::new(MockSttRpc {
            chain_id: CHAIN_ID_DEVNET,
            ..Default::default()
        });
        let state = Arc::new(StateFile::ephemeral());
        let submitter = Submitter::new(
            rpc.clone(),
            Keypair::from_seed(3),
            CHAIN_ID_DEVNET,
            state.clone(),
        );
        let params = ChainParams {
            confirmations_tron: 5,
            ..Default::default()
        };
        let ctx = DepositContext {
            submitter: &submitter,
            state: &state,
            params: &params,
        };
        let cfg = TronConfig {
            api_url: String::new(),
            api_key: None,
            tokens: vec![],
            fee_sun: 1,
            fee_limit_sun: 1,
            hot_index: 0,
        };
        assert_eq!(scan_tron(&ctx, &mock, &book, &cfg).await.unwrap(), 1);
        // The cursor stays at the still-shallow transfer so it is re-read later.
        assert_eq!(state.cursor("tron:min_timestamp"), Some(4_500_000));
        let submitted = rpc.submitted.lock().unwrap();
        let Action::ObserveDeposit(o) = &submitted[0].envelope.action else {
            panic!()
        };
        assert_eq!(o.amount, 7_000_000);
        assert_eq!(o.external_height, 990);
        assert_eq!(o.tip_height, 1000);
        assert_eq!(o.asset, Asset::vault("TRON", "TRX"));
        assert_eq!(o.proof, Proof::None);
    }
}
