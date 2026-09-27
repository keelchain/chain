//! Outbound processing: batched withdrawals → signed transactions →
//! broadcast → confirmation → `ObserveOutbound`.
//!
//! One signer per batch leads (`batch_id % signers.len()`); the others
//! look for the payment on chain and, after `leader_timeout_secs` per
//! position, take over in turn. Bitcoin batches every outbound of a batch
//! into one transaction and never builds a second one while a vault
//! transaction is unconfirmed (the one-in-flight guard from
//! docs/plan.md §4); Ethereum and Tron pay each outbound separately
//! from the hot address (index 0).

use crate::{
    chains::{
        btc::{self, BitcoinRpc},
        eth::{self, EthRpc},
        tron::{self, TronApi},
        AddressBook,
    },
    config::{BitcoinConfig, EthereumConfig, OutboundConfig, TronConfig},
    rpc::{ChainParams, OutboundRow, Submitter, VaultView},
    state::{now_unix, OutboundTx, StateFile},
    tss::TssClient,
};
use keel_actions::{Action, Chain, OutboundObservation};
use keel_chains::{btc::Utxo, eth::Eip1559Tx, policy::SignContext, tron::TronTxBuilder, Network};
use keel_types::Address;
use std::collections::BTreeMap;

pub struct OutboundContext<'a> {
    pub submitter: &'a Submitter,
    pub state: &'a StateFile,
    pub params: &'a ChainParams,
    pub tss: &'a dyn TssClient,
    pub cfg: &'a OutboundConfig,
}

/// Whether this observer should build the batch now: the leader at once,
/// the k-th next signer after k timeouts. `None` if not a signer.
pub fn takeover_delay(vault: &VaultView, me: &Address, batch_id: u64) -> Option<u64> {
    let n = vault.signers.len() as u64;
    let my_pos = vault.signers.iter().position(|s| s == me)? as u64;
    let leader = batch_id % n;
    Some((my_pos + n - leader) % n)
}

impl OutboundContext<'_> {
    fn may_build(&self, vault: &VaultView, batch_id: u64) -> bool {
        if !self.cfg.enabled {
            return false;
        }
        let Some(k) = takeover_delay(vault, &self.submitter.address(), batch_id) else {
            return false;
        };
        if k == 0 {
            return true;
        }
        let first_seen = self.state.read(|d| d.first_seen.get(&batch_id).copied());
        match first_seen {
            Some(t) => now_unix().saturating_sub(t) >= k * self.cfg.leader_timeout_secs,
            None => {
                let _ = self.state.update(|d| {
                    d.first_seen.insert(batch_id, now_unix());
                });
                false
            }
        }
    }

    fn known_tx(&self, id: u64) -> Option<OutboundTx> {
        self.state.read(|d| d.outbound_txs.get(&id).cloned())
    }

    fn remember(
        &self,
        ids: &[u64],
        chain: Chain,
        txid: &str,
        fee_each: u128,
        ours: bool,
        raw: Option<String>,
    ) -> anyhow::Result<()> {
        self.state.update(|d| {
            for id in ids {
                d.outbound_txs.insert(
                    *id,
                    OutboundTx {
                        chain: chain.as_str().into(),
                        txid: txid.to_string(),
                        broadcast_unix_secs: now_unix(),
                        fee_hint: fee_each,
                        ours,
                        raw: raw.clone(),
                    },
                );
            }
        })
    }

    async fn observe(
        &self,
        id: u64,
        tx_hash: [u8; 32],
        external_height: u64,
        tip_height: u64,
        fee_paid: u128,
        success: bool,
    ) -> anyhow::Result<()> {
        let key = format!("outbound:{id}");
        let res = self
            .submitter
            .submit(
                Some(&key),
                Action::ObserveOutbound(OutboundObservation {
                    outbound_id: id,
                    tx_hash,
                    external_height,
                    tip_height,
                    fee_paid,
                    success,
                }),
            )
            .await?;
        if res.admitted {
            tracing::info!(
                id,
                tx = hex::encode(tx_hash),
                success,
                "outbound observation submitted"
            );
        }
        Ok(())
    }
}

/// The batches of `chain` drawn from one vault: the network's (`None`)
/// or a client's.
fn batches_of(
    rows: &[OutboundRow],
    chain: Chain,
    custodian: Option<Address>,
) -> BTreeMap<u64, Vec<OutboundRow>> {
    let rows: Vec<OutboundRow> = rows
        .iter()
        .filter(|r| r.custodian == custodian)
        .cloned()
        .collect();
    let rows = &rows[..];
    let mut out: BTreeMap<u64, Vec<OutboundRow>> = BTreeMap::new();
    for r in rows
        .iter()
        .filter(|r| r.chain == chain && r.status == "Batched")
    {
        out.entry(r.batch_id.unwrap_or(r.id))
            .or_default()
            .push(r.clone());
    }
    out
}

fn token_contract<'a>(tokens: &'a [crate::config::TokenConfig], asset: &str) -> Option<&'a str> {
    let symbol = asset.split_once('.').map(|(_, s)| s).unwrap_or(asset);
    tokens
        .iter()
        .find(|t| t.symbol == symbol)
        .map(|t| t.contract.as_str())
}

// ---------------------------------------------------------------- bitcoin

/// Build and sign one batch transaction: (raw, txid, fee).
pub async fn assemble_btc(
    book: &AddressBook,
    utxos: &[Utxo],
    outputs: &[(String, u64)],
    fee_rate: u64,
    tss: &dyn TssClient,
    network: Network,
    batch_id: u64,
) -> anyhow::Result<(Vec<u8>, [u8; 32], u64)> {
    let change = book
        .address(0)
        .ok_or_else(|| anyhow::anyhow!("no hot address"))?;
    let unsigned = keel_chains::btc::build_batch(utxos, outputs, change, fee_rate, network)?;
    // Every signer checks this description against the chain before signing.
    let raw_unsigned = hex::encode(bitcoin::consensus::encode::serialize(&unsigned.tx));
    let mut sigs = Vec::with_capacity(unsigned.sighashes.len());
    for s in &unsigned.sighashes {
        let context = SignContext::Btc {
            batch_id,
            raw_tx: raw_unsigned.clone(),
            input: s.input as u32,
            prevout_value: unsigned.selected[s.input].value,
            prevout_pubkey: hex::encode(s.pubkey),
            network,
        };
        let sig = tss
            .sign(s.digest, &book.path(s.key_index), Some(&context))
            .await?;
        sigs.push(sig.compact());
    }
    let (raw, txid) = keel_chains::btc::finalize(&unsigned, &sigs)?;
    Ok((raw, txid, unsigned.fee))
}

pub async fn process_bitcoin(
    ctx: &OutboundContext<'_>,
    rpc: &dyn BitcoinRpc,
    book: &AddressBook,
    cfg: &BitcoinConfig,
    rows: &[OutboundRow],
) -> anyhow::Result<()> {
    let required = ctx.params.confirmations(Chain::Bitcoin) as u64;
    for (batch_id, outbounds) in batches_of(rows, Chain::Bitcoin, book.vault.custodian) {
        // Already broadcast (by us or found on chain): watch confirmations.
        if let Some(tx) = ctx.known_tx(outbounds[0].id) {
            match btc::tx_confirmations(rpc, &tx.txid).await? {
                Some((height, conf)) if conf > required => {
                    let tip = height + conf - 1;
                    let hash = bitcoin::Txid::from_str_hex(&tx.txid)?;
                    // The fee actually paid, split over the batch; the hint
                    // recorded at broadcast when the wallet cannot tell.
                    let paid = btc::tx_fee_sats(rpc, &tx.txid)
                        .await?
                        .map(|f| (f / outbounds.len() as u64) as u128);
                    for o in &outbounds {
                        let fee = paid.unwrap_or_else(|| {
                            ctx.known_tx(o.id)
                                .map(|t| t.fee_hint)
                                .unwrap_or(tx.fee_hint)
                        });
                        ctx.observe(o.id, *hash.as_ref(), height, tip, fee, true)
                            .await?;
                    }
                }
                Some(_) => {}
                None => {
                    // Ours but neither confirmed nor in the mempool: the
                    // broadcast was lost (node restart, network error).
                    if let (true, Some(raw)) = (tx.ours, &tx.raw) {
                        if !btc::in_mempool(rpc, &tx.txid).await {
                            match btc::send_raw(rpc, &hex::decode(raw)?).await {
                                Ok(sent) => {
                                    tracing::info!(batch_id, txid = %sent, "bitcoin batch rebroadcast")
                                }
                                Err(e) => {
                                    tracing::warn!(batch_id, txid = %tx.txid, error = %e, "bitcoin rebroadcast failed")
                                }
                            }
                        }
                    }
                }
            }
            continue;
        }
        // Followers: look for the payment.
        if !ctx.may_build(&book.vault, batch_id) {
            for o in &outbounds {
                if let Some(txid) = btc::find_payment(rpc, &o.to, o.amount as u64).await? {
                    ctx.remember(&[o.id], Chain::Bitcoin, &txid, o.fee_estimate, false, None)?;
                }
            }
            continue;
        }
        // One in flight: any unconfirmed vault transaction blocks new ones.
        let in_flight = ctx.state.read(|d| {
            d.outbound_txs
                .values()
                .filter(|t| t.chain == "BTC" && t.ours)
                .map(|t| t.txid.clone())
                .collect::<Vec<_>>()
        });
        let mut blocked = false;
        for txid in in_flight {
            if btc::tx_confirmations(rpc, &txid).await?.is_none() {
                blocked = true;
            }
        }
        if blocked {
            tracing::info!(
                batch_id,
                "bitcoin batch waits for the in-flight transaction"
            );
            continue;
        }
        let utxos = btc::vault_utxos(&btc::list_unspent(rpc, 1).await?, book)?;
        let outputs: Vec<(String, u64)> = outbounds
            .iter()
            .map(|o| (o.to.clone(), o.amount as u64))
            .collect();
        let fee_rate = btc::fee_rate(rpc, cfg.fee_target_blocks, cfg.fallback_sat_per_vb).await?;
        let (raw, txid, fee) = match assemble_btc(
            book,
            &utxos,
            &outputs,
            fee_rate,
            ctx.tss,
            cfg.network,
            batch_id,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(batch_id, error = %e, "cannot build bitcoin batch");
                continue;
            }
        };
        let txid_hex = bitcoin::Txid::from_slice_hex_free(&txid);
        let ids: Vec<u64> = outbounds.iter().map(|o| o.id).collect();
        ctx.remember(
            &ids,
            Chain::Bitcoin,
            &txid_hex,
            (fee / ids.len() as u64) as u128,
            true,
            Some(hex::encode(&raw)),
        )?;
        let sent = btc::send_raw(rpc, &raw).await?;
        tracing::info!(batch_id, txid = %sent, fee, "bitcoin batch broadcast");
    }
    Ok(())
}

trait TxidHex {
    fn from_str_hex(s: &str) -> anyhow::Result<bitcoin::Txid>;
    fn from_slice_hex_free(b: &[u8; 32]) -> String;
}

impl TxidHex for bitcoin::Txid {
    fn from_str_hex(s: &str) -> anyhow::Result<bitcoin::Txid> {
        Ok(s.parse()?)
    }

    fn from_slice_hex_free(b: &[u8; 32]) -> String {
        <bitcoin::Txid as bitcoin::hashes::Hash>::from_byte_array(*b).to_string()
    }
}

// ---------------------------------------------------------------- ethereum

/// Build and sign one Ethereum transfer: (raw, tx hash).
#[allow(clippy::too_many_arguments)]
pub async fn assemble_eth(
    book: &AddressBook,
    chain_id: u64,
    nonce: u64,
    token: Option<[u8; 20]>,
    to: [u8; 20],
    amount: u128,
    max_fee: u128,
    priority: u128,
    tss: &dyn TssClient,
    hot_index: u64,
    batch_id: u64,
) -> anyhow::Result<(Vec<u8>, [u8; 32], Eip1559Tx)> {
    let tx = match token {
        Some(t) => Eip1559Tx::erc20_transfer(chain_id, nonce, t, to, amount, max_fee, priority),
        None => Eip1559Tx::native_transfer(chain_id, nonce, to, amount, max_fee, priority),
    };
    let digest = tx.signing_hash();
    let context = SignContext::Eth {
        batch_id,
        chain_id: tx.chain_id,
        nonce: tx.nonce,
        max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
        max_fee_per_gas: tx.max_fee_per_gas,
        gas_limit: tx.gas_limit,
        to: format!("0x{}", hex::encode(tx.to)),
        value: tx.value,
        data: hex::encode(&tx.data),
    };
    let sig = tss
        .sign(digest, &book.path(hot_index), Some(&context))
        .await?;
    let hot_pubkey = book.pubkey(hot_index)?;
    let v = keel_chains::eth::recovery_id(&digest, &sig.compact(), &hot_pubkey)?;
    let raw = tx.raw_signed(&sig.compact(), v);
    Ok((raw.clone(), keel_chains::eth::tx_hash(&raw), tx))
}

pub async fn process_ethereum(
    ctx: &OutboundContext<'_>,
    rpc: &dyn EthRpc,
    book: &AddressBook,
    cfg: &EthereumConfig,
    rows: &[OutboundRow],
) -> anyhow::Result<()> {
    let required = ctx.params.confirmations(Chain::Ethereum) as u64;
    let hot = book
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    let hot_account = keel_chains::address::eth_decode(&hot)?;
    let tip = eth::block_number(rpc).await?;
    for (batch_id, outbounds) in batches_of(rows, Chain::Ethereum, book.vault.custodian) {
        for o in &outbounds {
            if let Some(tx) = ctx.known_tx(o.id) {
                if let Some(r) = eth::receipt_status(rpc, &tx.txid).await? {
                    if tip.saturating_sub(r.block_number) >= required {
                        let hash: [u8; 32] = hex::decode(tx.txid.trim_start_matches("0x"))?
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("tx hash length"))?;
                        ctx.observe(o.id, hash, r.block_number, tip, r.fee_paid, r.success)
                            .await?;
                    }
                }
                continue;
            }
            let token = if o.asset == "ETH.ETH" {
                None
            } else {
                Some(
                    token_contract(&cfg.tokens, &o.asset)
                        .ok_or_else(|| anyhow::anyhow!("no contract configured for {}", o.asset))?,
                )
            };
            if !ctx.may_build(&book.vault, batch_id) {
                let from_block = tip.saturating_sub(cfg.log_window);
                if let Some(txid) =
                    eth::find_payment(rpc, token, &hot, &o.to, o.amount, from_block, tip).await?
                {
                    ctx.remember(&[o.id], Chain::Ethereum, &txid, o.fee_estimate, false, None)?;
                }
                continue;
            }
            let chain_id = eth::chain_id(rpc).await?;
            let nonce = eth::nonce(rpc, &hot).await?;
            let (max_fee, priority) = eth::fee_params(rpc, cfg.fallback_priority_wei).await?;
            let to = keel_chains::address::eth_decode(&o.to)?;
            let token_account = match token {
                Some(t) => Some(keel_chains::address::eth_decode(t)?),
                None => None,
            };
            let (raw, hash, tx) = assemble_eth(
                book,
                chain_id,
                nonce,
                token_account,
                to,
                o.amount,
                max_fee,
                priority,
                ctx.tss,
                cfg.hot_index,
                o.batch_id.unwrap_or(0),
            )
            .await?;
            let _ = hot_account;
            let txid = format!("0x{}", hex::encode(hash));
            ctx.remember(
                &[o.id],
                Chain::Ethereum,
                &txid,
                tx.max_gas_cost(),
                true,
                Some(hex::encode(&raw)),
            )?;
            let sent = eth::send_raw(rpc, &raw).await?;
            tracing::info!(id = o.id, tx = %sent, "ethereum outbound broadcast");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- tron

/// Build and sign one Tron transfer: (broadcast JSON, txid).
#[allow(clippy::too_many_arguments)]
pub async fn assemble_tron(
    book: &AddressBook,
    now: &tron::NowBlock,
    token: Option<&str>,
    to: &str,
    amount: u128,
    fee_limit_sun: u64,
    tss: &dyn TssClient,
    hot_index: u64,
    batch_id: u64,
) -> anyhow::Result<(serde_json::Value, [u8; 32])> {
    let owner = keel_chains::address::tron_decode(
        book.address(hot_index)
            .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?,
    )?;
    let dest = keel_chains::address::tron_decode(to)?;
    let (ref_block_bytes, ref_block_hash) = keel_chains::tron::ref_block(now.number, &now.id);
    let timestamp_ms = now.timestamp_ms.max(now_unix() * 1000);
    let builder = TronTxBuilder {
        ref_block_bytes,
        ref_block_hash,
        timestamp_ms,
        expiration_ms: timestamp_ms + 60_000,
        fee_limit_sun,
    };
    let raw = match token {
        Some(t) => builder.trc20_transfer(
            &owner,
            &keel_chains::address::tron_decode(t)?,
            &dest,
            amount,
        ),
        None => builder.transfer(&owner, &dest, amount as u64),
    };
    let digest = raw.txid();
    let context = SignContext::Tron {
        batch_id,
        raw_data: hex::encode(raw.to_bytes()),
    };
    let sig = tss
        .sign(digest, &book.path(hot_index), Some(&context))
        .await?;
    let v = keel_chains::eth::recovery_id(&digest, &sig.compact(), &book.pubkey(hot_index)?)?;
    let sig65 = keel_chains::tron::Raw::signature(&sig.compact(), v);
    Ok((raw.broadcast_json(&sig65), digest))
}

pub async fn process_tron(
    ctx: &OutboundContext<'_>,
    api: &dyn TronApi,
    book: &AddressBook,
    cfg: &TronConfig,
    rows: &[OutboundRow],
) -> anyhow::Result<()> {
    let required = ctx.params.confirmations(Chain::Tron) as u64;
    let now = tron::now_block(api).await?;
    let hot = book
        .address(cfg.hot_index)
        .ok_or_else(|| anyhow::anyhow!("hot address not derived"))?
        .to_string();
    for (batch_id, outbounds) in batches_of(rows, Chain::Tron, book.vault.custodian) {
        for o in &outbounds {
            if let Some(tx) = ctx.known_tx(o.id) {
                if let Some(info) = tron::tx_info(api, &tx.txid).await? {
                    if now.number.saturating_sub(info.block_number) >= required {
                        let hash: [u8; 32] = hex::decode(&tx.txid)?
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("txid length"))?;
                        ctx.observe(
                            o.id,
                            hash,
                            info.block_number,
                            now.number,
                            info.fee_sun as u128,
                            info.success,
                        )
                        .await?;
                    }
                }
                continue;
            }
            let token = if o.asset == "TRON.TRX" {
                None
            } else {
                Some(
                    token_contract(&cfg.tokens, &o.asset)
                        .ok_or_else(|| anyhow::anyhow!("no contract configured for {}", o.asset))?,
                )
            };
            if !ctx.may_build(&book.vault, batch_id) {
                let since = now.timestamp_ms.saturating_sub(24 * 3600 * 1000);
                if let Some(txid) =
                    tron::find_payment(api, token, &hot, &o.to, o.amount, since).await?
                {
                    ctx.remember(&[o.id], Chain::Tron, &txid, o.fee_estimate, false, None)?;
                }
                continue;
            }
            let (json, txid) = assemble_tron(
                book,
                &now,
                token,
                &o.to,
                o.amount,
                cfg.fee_limit_sun,
                ctx.tss,
                cfg.hot_index,
                o.batch_id.unwrap_or(0),
            )
            .await?;
            ctx.remember(
                &[o.id],
                Chain::Tron,
                &hex::encode(txid),
                cfg.fee_sun as u128,
                true,
                Some(json.to_string()),
            )?;
            let sent = tron::broadcast(api, json).await?;
            tracing::info!(id = o.id, tx = %sent, "tron outbound broadcast");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tss::LocalSigner;
    use bitcoin::hashes::Hash as _;
    use keel_crypto::Keypair;
    use std::collections::BTreeMap;

    fn setup(chain: Chain) -> (LocalSigner, AddressBook) {
        let signer = LocalSigner::from_seed(&[2u8; 32]).unwrap();
        let vault = VaultView {
            chain,
            epoch: 1,
            public_key: signer.public_key(),
            chain_code: signer.chain_code(),
            signers: vec![
                Keypair::from_seed(1).address(),
                Keypair::from_seed(2).address(),
                Keypair::from_seed(3).address(),
            ],
            threshold: 2,
            next_deposit_index: 4,
            owners: BTreeMap::new(),
            custodian: None,
            signer_url: None,
        };
        let network = if chain == Chain::Bitcoin {
            Network::Regtest
        } else {
            Network::Mainnet
        };
        (signer, AddressBook::build(vault, network).unwrap())
    }

    #[test]
    fn leader_rotates_per_batch() {
        let (_, book) = setup(Chain::Bitcoin);
        let me = Keypair::from_seed(2).address();
        assert_eq!(takeover_delay(&book.vault, &me, 1), Some(0));
        assert_eq!(takeover_delay(&book.vault, &me, 2), Some(2));
        assert_eq!(takeover_delay(&book.vault, &me, 3), Some(1));
        assert_eq!(
            takeover_delay(&book.vault, &Keypair::from_seed(9).address(), 1),
            None
        );
    }

    #[tokio::test]
    async fn bitcoin_batch_from_mock_tss_verifies() {
        let (signer, book) = setup(Chain::Bitcoin);
        let utxos = vec![
            Utxo {
                txid: [1; 32],
                vout: 0,
                value: 300_000,
                key_index: 1,
                pubkey: book.pubkey(1).unwrap(),
            },
            Utxo {
                txid: [2; 32],
                vout: 1,
                value: 100_000,
                key_index: 2,
                pubkey: book.pubkey(2).unwrap(),
            },
        ];
        let dest =
            keel_chains::address::btc_address(&book.pubkey(3).unwrap(), Network::Regtest).unwrap();
        let (raw, txid, fee) = assemble_btc(
            &book,
            &utxos,
            &[(dest.clone(), 350_000)],
            5,
            &signer,
            Network::Regtest,
            1,
        )
        .await
        .unwrap();
        let tx: bitcoin::Transaction = bitcoin::consensus::deserialize(&raw).unwrap();
        assert_eq!(tx.compute_txid().to_byte_array(), txid);
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output.len(), 2); // payout + change to index 0
        assert!(fee > 0);
        let secp = secp256k1::Secp256k1::verification_only();
        let mut cache = bitcoin::sighash::SighashCache::new(&tx);
        for (i, u) in [&utxos[0], &utxos[1]].iter().enumerate() {
            let pk = bitcoin::CompressedPublicKey::from_slice(&u.pubkey).unwrap();
            let spk = bitcoin::ScriptBuf::new_p2wpkh(&pk.wpubkey_hash());
            let h = cache
                .p2wpkh_signature_hash(
                    i,
                    &spk,
                    bitcoin::Amount::from_sat(u.value),
                    bitcoin::sighash::EcdsaSighashType::All,
                )
                .unwrap();
            let w = &tx.input[i].witness;
            let sig = bitcoin::ecdsa::Signature::from_slice(&w[0]).unwrap();
            secp.verify_ecdsa(
                &secp256k1::Message::from_digest(h.to_byte_array()),
                &sig.signature,
                &secp256k1::PublicKey::from_slice(&w[1]).unwrap(),
            )
            .unwrap();
        }
        let change_spk = book
            .address(0)
            .unwrap()
            .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
            .unwrap()
            .assume_checked()
            .script_pubkey();
        assert!(tx.output.iter().any(|o| o.script_pubkey == change_spk));
    }

    #[tokio::test]
    async fn ethereum_and_tron_transfers_recover_the_hot_key() {
        let (signer, book) = setup(Chain::Ethereum);
        let (raw, hash, tx) = assemble_eth(
            &book,
            31337,
            4,
            Some([0xaa; 20]),
            [0xbb; 20],
            1_000_000,
            30_000_000_000,
            1_000_000_000,
            &signer,
            0,
            1,
        )
        .await
        .unwrap();
        assert_eq!(raw[0], 0x02);
        assert_eq!(hash, keel_chains::eth::tx_hash(&raw));
        assert_eq!(tx.nonce, 4);
        assert_eq!(tx.to, [0xaa; 20]);
        // Recover the signer from the raw tx: the signature covers the signing hash.
        let digest = tx.signing_hash();
        let sig = signer.sign_sync(digest, &book.path(0)).unwrap();
        assert_eq!(
            keel_chains::eth::recovery_id(&digest, &sig.compact(), &book.pubkey(0).unwrap())
                .unwrap(),
            sig.v
        );

        let (signer, book) = setup(Chain::Tron);
        let now = tron::NowBlock {
            number: 55_000_048,
            id: [7u8; 32],
            timestamp_ms: 1_700_000_000_000,
        };
        let usdt = "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t";
        let (json, txid) = assemble_tron(
            &book,
            &now,
            Some(usdt),
            book.address(3).unwrap(),
            5_000_000,
            100_000_000,
            &signer,
            0,
            1,
        )
        .await
        .unwrap();
        assert_eq!(json["txID"].as_str().unwrap(), hex::encode(txid));
        let sig65 = hex::decode(json["signature"][0].as_str().unwrap()).unwrap();
        assert_eq!(sig65.len(), 65);
        let raw = hex::decode(json["raw_data_hex"].as_str().unwrap()).unwrap();
        assert_eq!(
            <sha2::Sha256 as sha2::Digest>::digest(&raw).as_slice(),
            &txid
        );
        let mut sig64 = [0u8; 64];
        sig64.copy_from_slice(&sig65[..64]);
        assert_eq!(
            keel_chains::eth::recovery_id(&txid, &sig64, &book.pubkey(0).unwrap()).unwrap(),
            sig65[64] - 27
        );
        let (json_trx, _) = assemble_tron(
            &book,
            &now,
            None,
            book.address(3).unwrap(),
            1_000_000,
            100_000_000,
            &signer,
            0,
            1,
        )
        .await
        .unwrap();
        assert_ne!(json_trx["raw_data_hex"], json["raw_data_hex"]);
    }
}
