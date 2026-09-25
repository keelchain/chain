//! Bitcoin: batched P2WPKH spends from vault-derived keys. Builds the
//! unsigned transaction and the per-input sighashes; the caller obtains
//! TSS signatures and calls [`finalize`].

use crate::Error;
use bitcoin::{
    absolute::LockTime,
    consensus::encode::serialize,
    hashes::Hash,
    secp256k1::ecdsa::Signature,
    sighash::{EcdsaSighashType, SighashCache},
    transaction::Version,
    Address, Amount, CompressedPublicKey, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
    Txid, Witness,
};
use std::str::FromStr;

/// Dust threshold for P2WPKH change (sats).
pub const DUST: u64 = 546;
const VB_BASE: u64 = 11;
const VB_INPUT: u64 = 68;
const VB_OUTPUT: u64 = 34;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Utxo {
    pub txid: [u8; 32],
    pub vout: u32,
    pub value: u64,
    /// Deposit index whose derived key controls this output.
    pub key_index: u64,
    pub pubkey: [u8; 33],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSighash {
    pub input: usize,
    pub digest: [u8; 32],
    pub key_index: u64,
    pub pubkey: [u8; 33],
}

#[derive(Clone, Debug)]
pub struct UnsignedBtc {
    pub tx: Transaction,
    pub sighashes: Vec<InputSighash>,
    pub selected: Vec<Utxo>,
    pub fee: u64,
    pub change: u64,
    pub vsize_estimate: u64,
}

fn p2wpkh_script(pubkey: &[u8; 33]) -> Result<ScriptBuf, Error> {
    let pk = CompressedPublicKey::from_slice(pubkey).map_err(|e| Error::Key(e.to_string()))?;
    Ok(ScriptBuf::new_p2wpkh(&pk.wpubkey_hash()))
}

/// Build a batch payout. Coin selection is largest-first; change below dust
/// is folded into the fee.
pub fn build_batch(
    utxos: &[Utxo],
    outputs: &[(String, u64)],
    change_address: &str,
    fee_rate_sat_vb: u64,
    network: crate::Network,
) -> Result<UnsignedBtc, Error> {
    if outputs.is_empty() {
        return Err(Error::Tx("no outputs".into()));
    }
    let net = network.bitcoin();
    let mut txouts = Vec::with_capacity(outputs.len() + 1);
    let mut total_out: u64 = 0;
    for (addr, value) in outputs {
        if *value < DUST {
            return Err(Error::Tx(format!("output {addr} below dust")));
        }
        let a = Address::from_str(addr)
            .map_err(|e| Error::Address(e.to_string()))?
            .require_network(net)
            .map_err(|e| Error::Address(e.to_string()))?;
        txouts.push(TxOut {
            value: Amount::from_sat(*value),
            script_pubkey: a.script_pubkey(),
        });
        total_out = total_out
            .checked_add(*value)
            .ok_or_else(|| Error::Tx("overflow".into()))?;
    }
    let change_addr = Address::from_str(change_address)
        .map_err(|e| Error::Address(e.to_string()))?
        .require_network(net)
        .map_err(|e| Error::Address(e.to_string()))?;

    let mut sorted: Vec<Utxo> = utxos.to_vec();
    sorted.sort_by(|a, b| {
        b.value
            .cmp(&a.value)
            .then(a.txid.cmp(&b.txid))
            .then(a.vout.cmp(&b.vout))
    });
    let mut selected = Vec::new();
    let mut total_in: u64 = 0;
    let fee_for =
        |n_in: u64, n_out: u64| (VB_BASE + VB_INPUT * n_in + VB_OUTPUT * n_out) * fee_rate_sat_vb;
    let mut fee = 0;
    let mut change = 0;
    let mut covered = false;
    for u in sorted {
        total_in += u.value;
        selected.push(u);
        let n_in = selected.len() as u64;
        let n_out = txouts.len() as u64;
        // Try with change output first.
        let f_change = fee_for(n_in, n_out + 1);
        if total_in >= total_out + f_change + DUST {
            fee = f_change;
            change = total_in - total_out - f_change;
            covered = true;
            break;
        }
        let f_nochange = fee_for(n_in, n_out);
        if total_in >= total_out + f_nochange {
            fee = total_in - total_out; // any excess below dust burns into the fee
            change = 0;
            covered = true;
            break;
        }
    }
    if !covered {
        let need = total_out + fee_for(selected.len().max(1) as u64, txouts.len() as u64);
        return Err(Error::InsufficientFunds {
            need,
            have: total_in,
        });
    }
    if change > 0 {
        txouts.push(TxOut {
            value: Amount::from_sat(change),
            script_pubkey: change_addr.script_pubkey(),
        });
    }

    let inputs: Vec<TxIn> = selected
        .iter()
        .map(|u| TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array(u.txid),
                vout: u.vout,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        })
        .collect();
    let tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: inputs,
        output: txouts,
    };

    let mut cache = SighashCache::new(&tx);
    let mut sighashes = Vec::with_capacity(selected.len());
    for (i, u) in selected.iter().enumerate() {
        let spk = p2wpkh_script(&u.pubkey)?;
        let h = cache
            .p2wpkh_signature_hash(i, &spk, Amount::from_sat(u.value), EcdsaSighashType::All)
            .map_err(|e| Error::Tx(e.to_string()))?;
        sighashes.push(InputSighash {
            input: i,
            digest: h.to_byte_array(),
            key_index: u.key_index,
            pubkey: u.pubkey,
        });
    }
    let vsize_estimate =
        VB_BASE + VB_INPUT * selected.len() as u64 + VB_OUTPUT * tx.output.len() as u64;
    Ok(UnsignedBtc {
        tx,
        sighashes,
        selected,
        fee,
        change,
        vsize_estimate,
    })
}

/// Attach compact (r||s) signatures in input order; returns the raw tx and its txid.
pub fn finalize(unsigned: &UnsignedBtc, sigs64: &[[u8; 64]]) -> Result<(Vec<u8>, [u8; 32]), Error> {
    if sigs64.len() != unsigned.sighashes.len() {
        return Err(Error::Tx("signature count mismatch".into()));
    }
    let mut tx = unsigned.tx.clone();
    for (i, sig) in sigs64.iter().enumerate() {
        let mut s = Signature::from_compact(sig).map_err(|e| Error::Tx(e.to_string()))?;
        s.normalize_s();
        let sig = bitcoin::ecdsa::Signature {
            signature: s,
            sighash_type: EcdsaSighashType::All,
        };
        let pk = CompressedPublicKey::from_slice(&unsigned.sighashes[i].pubkey)
            .map_err(|e| Error::Key(e.to_string()))?;
        tx.input[i].witness = Witness::p2wpkh(&sig, &pk.0);
    }
    let txid = tx.compute_txid().to_byte_array();
    Ok((serialize(&tx), txid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Network;
    use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, SecretKey};

    fn key(n: u8) -> (SecretKey, [u8; 33]) {
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&[n; 32]).unwrap();
        (sk, PublicKey::from_secret_key(&secp, &sk).serialize())
    }

    fn addr(pk: &[u8; 33]) -> String {
        crate::address::btc_address(pk, Network::Regtest).unwrap()
    }

    #[test]
    fn selects_largest_first_pays_fee_and_returns_change() {
        let (_, pk1) = key(1);
        let (_, pk2) = key(2);
        let (_, dest) = key(9);
        let utxos = vec![
            Utxo {
                txid: [1; 32],
                vout: 0,
                value: 50_000,
                key_index: 1,
                pubkey: pk1,
            },
            Utxo {
                txid: [2; 32],
                vout: 1,
                value: 200_000,
                key_index: 2,
                pubkey: pk2,
            },
        ];
        let u = build_batch(
            &utxos,
            &[(addr(&dest), 100_000)],
            &addr(&pk1),
            10,
            Network::Regtest,
        )
        .unwrap();
        assert_eq!(u.selected.len(), 1);
        assert_eq!(u.selected[0].value, 200_000);
        assert_eq!(u.fee, (VB_BASE + VB_INPUT + 2 * VB_OUTPUT) * 10);
        assert_eq!(u.change, 200_000 - 100_000 - u.fee);
        assert_eq!(u.tx.output.len(), 2);
        assert_eq!(u.sighashes.len(), 1);
        assert_eq!(u.sighashes[0].key_index, 2);
    }

    #[test]
    fn insufficient_and_dust_change() {
        let (_, pk1) = key(1);
        let (_, dest) = key(9);
        let utxos = vec![Utxo {
            txid: [1; 32],
            vout: 0,
            value: 10_000,
            key_index: 1,
            pubkey: pk1,
        }];
        let err = build_batch(
            &utxos,
            &[(addr(&dest), 20_000)],
            &addr(&pk1),
            1,
            Network::Regtest,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InsufficientFunds { .. }));
        // Exactly enough with sub-dust remainder: no change output, remainder into fee.
        let fee = VB_BASE + VB_INPUT + VB_OUTPUT;
        let u = build_batch(
            &utxos,
            &[(addr(&dest), 10_000 - fee - 100)],
            &addr(&pk1),
            1,
            Network::Regtest,
        )
        .unwrap();
        assert_eq!(u.tx.output.len(), 1);
        assert_eq!(u.change, 0);
        assert_eq!(u.fee, fee + 100);
        assert!(build_batch(
            &utxos,
            &[(addr(&dest), 100)],
            &addr(&pk1),
            1,
            Network::Regtest
        )
        .is_err());
        assert!(build_batch(
            &utxos,
            &[("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4".into(), 1000)],
            &addr(&pk1),
            1,
            Network::Regtest
        )
        .is_err());
    }

    #[test]
    fn signed_batch_verifies_against_the_input_keys() {
        let secp = Secp256k1::new();
        let (sk1, pk1) = key(1);
        let (sk2, pk2) = key(2);
        let (_, d1) = key(9);
        let (_, d2) = key(8);
        let utxos = vec![
            Utxo {
                txid: [1; 32],
                vout: 0,
                value: 80_000,
                key_index: 1,
                pubkey: pk1,
            },
            Utxo {
                txid: [2; 32],
                vout: 3,
                value: 70_000,
                key_index: 2,
                pubkey: pk2,
            },
        ];
        let u = build_batch(
            &utxos,
            &[(addr(&d1), 60_000), (addr(&d2), 60_000)],
            &addr(&pk1),
            5,
            Network::Regtest,
        )
        .unwrap();
        assert_eq!(u.selected.len(), 2);
        let sigs: Vec<[u8; 64]> = u
            .sighashes
            .iter()
            .map(|s| {
                let sk = if s.key_index == 1 { sk1 } else { sk2 };
                secp.sign_ecdsa(&Message::from_digest(s.digest), &sk)
                    .serialize_compact()
            })
            .collect();
        let (raw, txid) = finalize(&u, &sigs).unwrap();
        let tx: Transaction = bitcoin::consensus::deserialize(&raw).unwrap();
        assert_eq!(tx.compute_txid().to_byte_array(), txid);
        assert_eq!(tx.input[0].witness.len(), 2);
        // Re-derive the sighash from the finalized tx and verify each witness.
        let mut cache = SighashCache::new(&tx);
        for (i, s) in u.sighashes.iter().enumerate() {
            let spk = p2wpkh_script(&s.pubkey).unwrap();
            let h = cache
                .p2wpkh_signature_hash(
                    i,
                    &spk,
                    Amount::from_sat(u.selected[i].value),
                    EcdsaSighashType::All,
                )
                .unwrap();
            let w = &tx.input[i].witness;
            let sig = bitcoin::ecdsa::Signature::from_slice(&w[0]).unwrap();
            let pk = PublicKey::from_slice(&w[1]).unwrap();
            secp.verify_ecdsa(
                &Message::from_digest(h.to_byte_array()),
                &sig.signature,
                &pk,
            )
            .unwrap();
        }
        assert!(finalize(&u, &sigs[..1]).is_err());
    }
}
