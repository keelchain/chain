//! Binding a signing request to a withdrawal batch the chain has finalized.
//!
//! A threshold signer must not sign whatever digest it is handed: the party
//! that assembles a transaction (an observer, or a client's own machine for
//! a client-owned vault) attaches a [`SignContext`] describing the raw
//! transaction and the batch it pays, and every signer runs [`verify`]
//! before joining the session. The check is local and needs only the
//! chain's outbound rows for that batch:
//!
//! - the digest is the signing hash of the described transaction, so the
//!   signature cannot be reused on any other transaction;
//! - every destination output pays an open outbound of the batch, with the
//!   exact amount; a Bitcoin batch pays every outbound of the batch and may
//!   return change only to the vault's own hot address;
//! - the key at the signing path is the vault key the context claims
//!   (the input's previous output on Bitcoin, the owner on Tron).
//!
//! What it does not check: fees (bounded by the network fee median the
//! chain records), the token contract of an ERC-20/TRC-20 transfer (the
//! chain maps assets to contracts; a wrong contract simply fails on the
//! target chain), and expirations.

use crate::{address, btc, eth::Eip1559Tx, hd, tron, Network};
use bitcoin::{
    consensus, hashes::Hash as _, sighash::EcdsaSighashType, sighash::SighashCache, Amount,
    Transaction,
};
use keel_actions::Chain;
use prost::Message as _;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// What a signing request is for. Attached by whoever assembles the
/// transaction; verified by every signer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "chain", rename_all = "UPPERCASE")]
pub enum SignContext {
    Btc {
        batch_id: u64,
        /// Unsigned transaction, consensus-serialized, hex.
        raw_tx: String,
        /// Input this digest signs.
        input: u32,
        /// The spent output: value and the compressed key its P2WPKH pays.
        prevout_value: u64,
        prevout_pubkey: String,
        network: Network,
    },
    Eth {
        batch_id: u64,
        chain_id: u64,
        nonce: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        gas_limit: u64,
        /// `0x`-prefixed 20-byte recipient (the token contract for ERC-20).
        to: String,
        value: u128,
        /// Call data, hex (empty for a native transfer).
        data: String,
    },
    Tron {
        batch_id: u64,
        /// `raw_data` protobuf, hex.
        raw_data: String,
    },
}

impl SignContext {
    pub fn batch_id(&self) -> u64 {
        match self {
            SignContext::Btc { batch_id, .. }
            | SignContext::Eth { batch_id, .. }
            | SignContext::Tron { batch_id, .. } => *batch_id,
        }
    }

    pub fn chain(&self) -> Chain {
        match self {
            SignContext::Btc { .. } => Chain::Bitcoin,
            SignContext::Eth { .. } => Chain::Ethereum,
            SignContext::Tron { .. } => Chain::Tron,
        }
    }
}

/// An outbound row as the node reports it (`GET /v1/vaults/outbounds`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundRef {
    pub id: u64,
    pub chain: Chain,
    pub to: String,
    pub amount: u128,
    pub status: String,
    pub batch_id: Option<u64>,
}

fn open_rows<'a>(
    rows: &'a [OutboundRef],
    ctx: &SignContext,
) -> Result<Vec<&'a OutboundRef>, String> {
    let list: Vec<&OutboundRef> = rows
        .iter()
        .filter(|r| r.chain == ctx.chain() && r.batch_id == Some(ctx.batch_id()))
        .collect();
    if list.is_empty() {
        return Err(format!(
            "batch {} has no outbound on {}",
            ctx.batch_id(),
            ctx.chain().as_str()
        ));
    }
    if let Some(r) = list
        .iter()
        .find(|r| !matches!(r.status.as_str(), "Queued" | "Batched"))
    {
        return Err(format!("outbound {} is {}, not open", r.id, r.status));
    }
    Ok(list)
}

/// Check that signing `digest` with the vault child at `path` is exactly
/// what `ctx` describes, and that the transaction pays open outbounds of
/// its batch. `rows` is the node's outbound list (any chain, any batch).
pub fn verify(
    ctx: &SignContext,
    digest: &[u8; 32],
    path: &[u32],
    vault_pubkey: &[u8; 33],
    chain_code: &[u8; 32],
    rows: &[OutboundRef],
) -> Result<(), String> {
    let rows = open_rows(rows, ctx)?;
    let key = hd::child_pubkey(vault_pubkey, chain_code, path)
        .map_err(|e| format!("signing path: {e}"))?
        .public_key;
    match ctx {
        SignContext::Btc {
            raw_tx,
            input,
            prevout_value,
            prevout_pubkey,
            network,
            ..
        } => {
            let prev: [u8; 33] = hex::decode(prevout_pubkey)
                .map_err(|e| format!("prevout pubkey: {e}"))?
                .try_into()
                .map_err(|_| "prevout pubkey: not 33 bytes".to_string())?;
            if prev != key {
                return Err("the signing path does not derive the spent output's key".into());
            }
            let raw = hex::decode(raw_tx).map_err(|e| format!("raw tx: {e}"))?;
            let tx: Transaction =
                consensus::deserialize(&raw).map_err(|e| format!("raw tx: {e}"))?;
            let i = *input as usize;
            if i >= tx.input.len() {
                return Err(format!(
                    "input {i} out of range ({} inputs)",
                    tx.input.len()
                ));
            }
            let spk = btc::p2wpkh_script(&prev).map_err(|e| e.to_string())?;
            let want = SighashCache::new(&tx)
                .p2wpkh_signature_hash(
                    i,
                    &spk,
                    Amount::from_sat(*prevout_value),
                    EcdsaSighashType::All,
                )
                .map_err(|e| e.to_string())?
                .to_byte_array();
            if want != *digest {
                return Err("digest is not the sighash of the described input".into());
            }
            let hot = hd::child_pubkey(
                vault_pubkey,
                chain_code,
                &hd::deposit_path(Chain::Bitcoin, 0),
            )
            .map_err(|e| e.to_string())?
            .public_key;
            let change_spk = btc::p2wpkh_script(&hot).map_err(|e| e.to_string())?;
            let net = network.bitcoin();
            let mut matched = vec![false; rows.len()];
            let mut change_outputs = 0;
            for out in &tx.output {
                if out.script_pubkey == change_spk {
                    change_outputs += 1;
                    if change_outputs > 1 {
                        return Err("more than one change output".into());
                    }
                    continue;
                }
                let hit = rows.iter().enumerate().find(|(j, r)| {
                    !matched[*j]
                        && r.amount == out.value.to_sat() as u128
                        && bitcoin::Address::from_str(&r.to)
                            .ok()
                            .and_then(|a| a.require_network(net).ok())
                            .is_some_and(|a| a.script_pubkey() == out.script_pubkey)
                });
                match hit {
                    Some((j, _)) => matched[j] = true,
                    None => {
                        return Err(format!(
                            "output of {} sat pays nothing in batch {}",
                            out.value.to_sat(),
                            ctx.batch_id()
                        ))
                    }
                }
            }
            if let Some(j) = matched.iter().position(|m| !m) {
                return Err(format!("outbound {} of the batch is not paid", rows[j].id));
            }
            Ok(())
        }
        SignContext::Eth {
            chain_id,
            nonce,
            max_priority_fee_per_gas,
            max_fee_per_gas,
            gas_limit,
            to,
            value,
            data,
            ..
        } => {
            let to20 = address::eth_decode(to).map_err(|e| format!("to: {e}"))?;
            let data =
                hex::decode(data.trim_start_matches("0x")).map_err(|e| format!("data: {e}"))?;
            let tx = Eip1559Tx {
                chain_id: *chain_id,
                nonce: *nonce,
                max_priority_fee_per_gas: *max_priority_fee_per_gas,
                max_fee_per_gas: *max_fee_per_gas,
                gas_limit: *gas_limit,
                to: to20,
                value: *value,
                data: data.clone(),
            };
            if tx.signing_hash() != *digest {
                return Err("digest is not the signing hash of the described transaction".into());
            }
            let (dest, amount) = if data.is_empty() {
                (to20, *value)
            } else {
                if *value != 0 {
                    return Err("token transfer with a native value".into());
                }
                parse_erc20_transfer(&data)?
            };
            let paid = rows
                .iter()
                .any(|r| r.amount == amount && address::eth_decode(&r.to).is_ok_and(|a| a == dest));
            if !paid {
                return Err(format!("transfer pays nothing in batch {}", ctx.batch_id()));
            }
            Ok(())
        }
        SignContext::Tron { raw_data, .. } => {
            let raw = hex::decode(raw_data).map_err(|e| format!("raw data: {e}"))?;
            let want: [u8; 32] = sha2::Sha256::digest_bytes(&raw);
            if want != *digest {
                return Err("digest is not the id of the described transaction".into());
            }
            let decoded =
                tron::Raw::decode(raw.as_slice()).map_err(|e| format!("raw data: {e}"))?;
            if decoded.encode_to_vec() != raw {
                return Err("raw data is not canonically encoded".into());
            }
            let contract = decoded
                .contract
                .first()
                .ok_or("transaction has no contract")?;
            if decoded.contract.len() != 1 {
                return Err("transaction has more than one contract".into());
            }
            let param = contract
                .parameter
                .as_ref()
                .ok_or("contract has no parameter")?;
            let owner_want =
                address::tron_decode(&address::tron_address(&key).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let (owner, dest, amount): (Vec<u8>, [u8; 21], u128) = match contract.r#type {
                tron::CONTRACT_TYPE_TRANSFER => {
                    let c = tron::TransferContract::decode(param.value.as_slice())
                        .map_err(|e| e.to_string())?;
                    let dest: [u8; 21] = c
                        .to_address
                        .clone()
                        .try_into()
                        .map_err(|_| "to address is not 21 bytes".to_string())?;
                    if c.amount < 0 {
                        return Err("negative amount".into());
                    }
                    (c.owner_address, dest, c.amount as u128)
                }
                tron::CONTRACT_TYPE_TRIGGER => {
                    let c = tron::TriggerSmartContract::decode(param.value.as_slice())
                        .map_err(|e| e.to_string())?;
                    if c.call_value != 0 {
                        return Err("token transfer with a native value".into());
                    }
                    let (to20, amount) = parse_erc20_transfer(&c.data)?;
                    let mut dest = [0x41u8; 21];
                    dest[1..].copy_from_slice(&to20);
                    (c.owner_address, dest, amount)
                }
                other => return Err(format!("unsupported contract type {other}")),
            };
            if owner.as_slice() != owner_want {
                return Err("the signing path does not derive the transaction's owner".into());
            }
            let paid = rows.iter().any(|r| {
                r.amount == amount && address::tron_decode(&r.to).is_ok_and(|a| a == dest)
            });
            if !paid {
                return Err(format!("transfer pays nothing in batch {}", ctx.batch_id()));
            }
            Ok(())
        }
    }
}

/// `transfer(address,uint256)` call data → (recipient, amount).
fn parse_erc20_transfer(data: &[u8]) -> Result<([u8; 20], u128), String> {
    if data.len() != 68 || data[..4] != [0xa9, 0x05, 0x9c, 0xbb] {
        return Err("call data is not transfer(address,uint256)".into());
    }
    if data[4..16].iter().any(|b| *b != 0) || data[36..52].iter().any(|b| *b != 0) {
        return Err("call data has out-of-range words".into());
    }
    let mut to = [0u8; 20];
    to.copy_from_slice(&data[16..36]);
    let mut amt = [0u8; 16];
    amt.copy_from_slice(&data[52..68]);
    Ok((to, u128::from_be_bytes(amt)))
}

trait DigestBytes {
    fn digest_bytes(data: &[u8]) -> [u8; 32];
}

impl DigestBytes for sha2::Sha256 {
    fn digest_bytes(data: &[u8]) -> [u8; 32] {
        use sha2::Digest as _;
        sha2::Sha256::digest(data).into()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::tron::TronTxBuilder;
    use bitcoin::{
        absolute::LockTime, transaction::Version, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Txid,
        Witness,
    };

    const VAULT: [u8; 33] = [
        0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
        0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16,
        0xf8, 0x17, 0x98,
    ];
    const CC: [u8; 32] = [9u8; 32];

    fn child(chain: Chain, i: u64) -> [u8; 33] {
        hd::child_pubkey(&VAULT, &CC, &hd::deposit_path(chain, i))
            .unwrap()
            .public_key
    }

    fn row(id: u64, chain: Chain, to: &str, amount: u128, batch: u64) -> OutboundRef {
        OutboundRef {
            id,
            chain,
            to: to.into(),
            amount,
            status: "Batched".into(),
            batch_id: Some(batch),
        }
    }

    fn btc_case() -> (
        SignContext,
        [u8; 32],
        Vec<u32>,
        Vec<OutboundRef>,
        Transaction,
    ) {
        let net = Network::Regtest;
        // A stranger's address (a key from another chain's path is as good
        // as any key that is not this vault's Bitcoin hot key).
        let dest = address::btc_address(&child(Chain::Tron, 3), net).unwrap();
        let spent_key = child(Chain::Bitcoin, 5);
        let path = hd::deposit_path(Chain::Bitcoin, 5);
        let change = btc::p2wpkh_script(&child(Chain::Bitcoin, 0)).unwrap();
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint {
                    txid: Txid::from_byte_array([1u8; 32]),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: bitcoin::Address::from_str(&dest)
                        .unwrap()
                        .require_network(net.bitcoin())
                        .unwrap()
                        .script_pubkey(),
                },
                TxOut {
                    value: Amount::from_sat(49_000),
                    script_pubkey: change,
                },
            ],
        };
        let spk = btc::p2wpkh_script(&spent_key).unwrap();
        let digest = SighashCache::new(&tx)
            .p2wpkh_signature_hash(0, &spk, Amount::from_sat(100_000), EcdsaSighashType::All)
            .unwrap()
            .to_byte_array();
        let ctx = SignContext::Btc {
            batch_id: 7,
            raw_tx: hex::encode(consensus::serialize(&tx)),
            input: 0,
            prevout_value: 100_000,
            prevout_pubkey: hex::encode(spent_key),
            network: net,
        };
        let rows = vec![
            row(1, Chain::Bitcoin, &dest, 50_000, 7),
            row(2, Chain::Tron, "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf", 5, 7),
        ];
        (ctx, digest, path, rows, tx)
    }

    #[test]
    fn bitcoin_batch_is_bound_to_its_outbounds() {
        let (ctx, digest, path, rows, tx) = btc_case();
        verify(&ctx, &digest, &path, &VAULT, &CC, &rows).unwrap();
        // Wrong digest.
        assert!(verify(&ctx, &[0u8; 32], &path, &VAULT, &CC, &rows).is_err());
        // Wrong path: the key does not match the spent output.
        let other = hd::deposit_path(Chain::Bitcoin, 6);
        assert!(verify(&ctx, &digest, &other, &VAULT, &CC, &rows).is_err());
        // An outbound of the batch left unpaid.
        let mut more = rows.clone();
        more.push(row(
            3,
            Chain::Bitcoin,
            &address::btc_address(&child(Chain::Bitcoin, 9), Network::Regtest).unwrap(),
            1_000,
            7,
        ));
        assert!(verify(&ctx, &digest, &path, &VAULT, &CC, &more).is_err());
        // A closed batch.
        let mut closed = rows.clone();
        closed[0].status = "Confirmed".into();
        assert!(verify(&ctx, &digest, &path, &VAULT, &CC, &closed).is_err());
        // An extra output to a stranger, with a matching digest.
        let mut bad = tx.clone();
        bad.output.push(TxOut {
            value: Amount::from_sat(1),
            script_pubkey: btc::p2wpkh_script(&child(Chain::Bitcoin, 9)).unwrap(),
        });
        let spk = btc::p2wpkh_script(&child(Chain::Bitcoin, 5)).unwrap();
        let d2 = SighashCache::new(&bad)
            .p2wpkh_signature_hash(0, &spk, Amount::from_sat(100_000), EcdsaSighashType::All)
            .unwrap()
            .to_byte_array();
        let SignContext::Btc {
            batch_id,
            input,
            prevout_value,
            prevout_pubkey,
            network,
            ..
        } = ctx.clone()
        else {
            unreachable!()
        };
        let ctx2 = SignContext::Btc {
            batch_id,
            raw_tx: hex::encode(consensus::serialize(&bad)),
            input,
            prevout_value,
            prevout_pubkey,
            network,
        };
        assert!(verify(&ctx2, &d2, &path, &VAULT, &CC, &rows).is_err());
    }

    #[test]
    fn ethereum_transfer_is_bound() {
        let to = child(Chain::Ethereum, 4);
        let to_addr = address::eth_address(&to).unwrap();
        let to20 = address::eth_decode(&to_addr).unwrap();
        let tx = Eip1559Tx::native_transfer(11155111, 3, to20, 1_000, 30, 2);
        let ctx = SignContext::Eth {
            batch_id: 2,
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
            max_fee_per_gas: tx.max_fee_per_gas,
            gas_limit: tx.gas_limit,
            to: to_addr.clone(),
            value: tx.value,
            data: String::new(),
        };
        let path = hd::deposit_path(Chain::Ethereum, 0);
        let rows = vec![row(1, Chain::Ethereum, &to_addr, 1_000, 2)];
        verify(&ctx, &tx.signing_hash(), &path, &VAULT, &CC, &rows).unwrap();
        let wrong = vec![row(1, Chain::Ethereum, &to_addr, 999, 2)];
        assert!(verify(&ctx, &tx.signing_hash(), &path, &VAULT, &CC, &wrong).is_err());
        // ERC-20.
        let token = [0x11u8; 20];
        let tx = Eip1559Tx::erc20_transfer(11155111, 4, token, to20, 5_000_000, 30, 2);
        let ctx = SignContext::Eth {
            batch_id: 3,
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
            max_fee_per_gas: tx.max_fee_per_gas,
            gas_limit: tx.gas_limit,
            to: format!("0x{}", hex::encode(token)),
            value: 0,
            data: hex::encode(&tx.data),
        };
        let rows = vec![row(9, Chain::Ethereum, &to_addr, 5_000_000, 3)];
        verify(&ctx, &tx.signing_hash(), &path, &VAULT, &CC, &rows).unwrap();
    }

    #[test]
    fn tron_transfer_is_bound_to_owner_and_outbound() {
        let owner_key = child(Chain::Tron, 0);
        let owner = address::tron_decode(&address::tron_address(&owner_key).unwrap()).unwrap();
        let dest_addr = address::tron_address(&child(Chain::Tron, 8)).unwrap();
        let dest = address::tron_decode(&dest_addr).unwrap();
        let b = TronTxBuilder {
            ref_block_bytes: [1, 2],
            ref_block_hash: [3u8; 8],
            timestamp_ms: 1_000,
            expiration_ms: 61_000,
            fee_limit_sun: 100,
        };
        let raw = b.transfer(&owner, &dest, 1_500_000);
        let ctx = SignContext::Tron {
            batch_id: 5,
            raw_data: hex::encode(raw.to_bytes()),
        };
        let path = hd::deposit_path(Chain::Tron, 0);
        let rows = vec![row(1, Chain::Tron, &dest_addr, 1_500_000, 5)];
        verify(&ctx, &raw.txid(), &path, &VAULT, &CC, &rows).unwrap();
        // Signing with another index: the owner does not match.
        assert!(verify(
            &ctx,
            &raw.txid(),
            &hd::deposit_path(Chain::Tron, 1),
            &VAULT,
            &CC,
            &rows
        )
        .is_err());
        // TRC-20 to the same recipient.
        let token = address::tron_decode("TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf").unwrap();
        let raw = b.trc20_transfer(&owner, &token, &dest, 2_000_000);
        let ctx = SignContext::Tron {
            batch_id: 6,
            raw_data: hex::encode(raw.to_bytes()),
        };
        let rows = vec![row(2, Chain::Tron, &dest_addr, 2_000_000, 6)];
        verify(&ctx, &raw.txid(), &path, &VAULT, &CC, &rows).unwrap();
        let rows = vec![row(2, Chain::Tron, &dest_addr, 2_000_001, 6)];
        assert!(verify(&ctx, &raw.txid(), &path, &VAULT, &CC, &rows).is_err());
    }
}
