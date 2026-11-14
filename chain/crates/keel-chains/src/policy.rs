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

use crate::{address, btc, eth::Eip1559Tx, hd, sol, tron, Network};
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
    /// A sweep of a draining custody generation (this key) to the active
    /// generation's own address (the `PoolRef` the signer fetches from the
    /// chain): one input of this key, one output, nothing else.
    BtcSweep {
        custodian: String,
        from_gen: u64,
        raw_tx: String,
        input: u32,
        prevout_value: u64,
        prevout_pubkey: String,
        network: Network,
    },
    /// Native or ERC-20 transfer from this key to the active generation's
    /// hot address.
    EthSweep {
        custodian: String,
        from_gen: u64,
        chain_id: u64,
        nonce: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        gas_limit: u64,
        to: String,
        value: u128,
        data: String,
    },
    /// Consolidation of a deposit index into a hot address: this key's own
    /// index 0, or the active generation's when `sweep`.
    EthConsolidate {
        custodian: String,
        index: u64,
        sweep: bool,
        chain_id: u64,
        nonce: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        gas_limit: u64,
        to: String,
        value: u128,
        data: String,
    },
    /// Native gas sent from this key's hot address to one of its deposit
    /// indexes so that index can be consolidated; capped.
    EthGasFund {
        custodian: String,
        index: u64,
        chain_id: u64,
        nonce: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        gas_limit: u64,
        to: String,
        value: u128,
    },
    TronSweep {
        custodian: String,
        from_gen: u64,
        raw_data: String,
    },
    TronConsolidate {
        custodian: String,
        index: u64,
        sweep: bool,
        raw_data: String,
    },
    TronGasFund {
        custodian: String,
        index: u64,
        raw_data: String,
    },
    /// A Solana payout: the legacy message (hex) paid by the hot address,
    /// and the SPL mint (base58) of every token asset the batch pays. The
    /// signer signs the message bytes; `digest` is their SHA-256.
    Sol {
        batch_id: u64,
        message: String,
        #[serde(default)]
        mints: Vec<(String, String)>,
    },
    /// Hot address of a draining generation to the active generation's.
    SolSweep {
        custodian: String,
        from_gen: u64,
        message: String,
    },
    /// Deposit index `index` to a hot address (the active generation's
    /// when `sweep`), paid by this key's own hot address.
    SolConsolidate {
        custodian: String,
        index: u64,
        sweep: bool,
        message: String,
    },
    /// A recovery (`GET /v1/recovery`, status `Open`): an EIP-1559
    /// transfer from deposit index `index` of the row's vault to the
    /// row's destination, on the row's EVM chain id.
    EthRecover {
        recovery_id: u64,
        index: u64,
        chain_id: u64,
        nonce: u64,
        max_priority_fee_per_gas: u128,
        max_fee_per_gas: u128,
        gas_limit: u64,
        to: String,
        value: u128,
        data: String,
    },
    /// A recovery on Tron: a transfer from the index's address.
    TronRecover {
        recovery_id: u64,
        index: u64,
        raw_data: String,
    },
    /// A recovery on Solana: a message moving the index's coins to the
    /// row's destination, paid by the index or by the vault's hot address.
    SolRecover {
        recovery_id: u64,
        index: u64,
        message: String,
    },
}

/// A recovery row as the node reports it (`GET /v1/recovery?status=Open`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRef {
    pub id: u64,
    pub chain: Chain,
    pub evm_chain_id: Option<u64>,
    pub token: Option<String>,
    pub to: String,
    pub deposit_index: u64,
    pub custodian: Option<String>,
    pub status: String,
}

/// The active generation of a custody vault, as the signer fetches it from
/// the chain (`GET /v1/custody/{custodian}`): where a sweep may send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolRef {
    /// 33 bytes (secp256k1) or 32 (Ed25519), as registered.
    pub active_pubkey: Vec<u8>,
    pub active_chain_code: [u8; 32],
}

/// Most native value one gas-funding transfer may carry.
pub const ETH_GAS_FUND_CAP_WEI: u128 = 10_000_000_000_000_000; // 0.01 ETH
pub const TRON_GAS_FUND_CAP_SUN: u128 = 50_000_000; // 50 TRX

impl SignContext {
    /// The batch a payout context pays; `None` for sweeps, consolidations
    /// and gas funding, which pay no outbound row.
    pub fn batch_id(&self) -> Option<u64> {
        match self {
            SignContext::Btc { batch_id, .. }
            | SignContext::Eth { batch_id, .. }
            | SignContext::Tron { batch_id, .. }
            | SignContext::Sol { batch_id, .. } => Some(*batch_id),
            _ => None,
        }
    }

    /// The EVM contexts serve Ethereum and BSC alike; the chain id they
    /// carry (bound into the signed digest) says which.
    pub fn chain(&self) -> Chain {
        match self {
            SignContext::Btc { .. } | SignContext::BtcSweep { .. } => Chain::Bitcoin,
            SignContext::Eth { chain_id, .. }
            | SignContext::EthSweep { chain_id, .. }
            | SignContext::EthConsolidate { chain_id, .. }
            | SignContext::EthGasFund { chain_id, .. } => crate::evm_chain_of_id(*chain_id),
            SignContext::Tron { .. }
            | SignContext::TronSweep { .. }
            | SignContext::TronConsolidate { .. }
            | SignContext::TronGasFund { .. } => Chain::Tron,
            SignContext::Sol { .. }
            | SignContext::SolSweep { .. }
            | SignContext::SolConsolidate { .. }
            | SignContext::SolRecover { .. } => Chain::Solana,
            SignContext::EthRecover { chain_id, .. } => crate::evm_chain_of_id(*chain_id),
            SignContext::TronRecover { .. } => Chain::Tron,
        }
    }

    /// The recovery row a recovery context serves.
    pub fn recovery_id(&self) -> Option<u64> {
        match self {
            SignContext::EthRecover { recovery_id, .. }
            | SignContext::TronRecover { recovery_id, .. }
            | SignContext::SolRecover { recovery_id, .. } => Some(*recovery_id),
            _ => None,
        }
    }

    /// The message bytes an Ed25519 context signs (none for ECDSA).
    pub fn message(&self) -> Option<&str> {
        match self {
            SignContext::Sol { message, .. }
            | SignContext::SolSweep { message, .. }
            | SignContext::SolConsolidate { message, .. }
            | SignContext::SolRecover { message, .. } => Some(message),
            _ => None,
        }
    }

    /// The custody vault a sweep / consolidation / gas-fund context is for.
    pub fn custodian(&self) -> Option<&str> {
        match self {
            SignContext::BtcSweep { custodian, .. }
            | SignContext::EthSweep { custodian, .. }
            | SignContext::EthConsolidate { custodian, .. }
            | SignContext::EthGasFund { custodian, .. }
            | SignContext::TronSweep { custodian, .. }
            | SignContext::TronConsolidate { custodian, .. }
            | SignContext::TronGasFund { custodian, .. }
            | SignContext::SolSweep { custodian, .. }
            | SignContext::SolConsolidate { custodian, .. } => Some(custodian),
            _ => None,
        }
    }

    /// Whether the destination is the *active* generation of the pool
    /// (a sweep, or a consolidation with `sweep`), which needs a `PoolRef`.
    pub fn needs_pool(&self) -> bool {
        matches!(
            self,
            SignContext::BtcSweep { .. }
                | SignContext::EthSweep { .. }
                | SignContext::TronSweep { .. }
                | SignContext::SolSweep { .. }
                | SignContext::EthConsolidate { sweep: true, .. }
                | SignContext::TronConsolidate { sweep: true, .. }
                | SignContext::SolConsolidate { sweep: true, .. }
        )
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
    /// The asset paid (`SOL.USDC`); Solana needs it to tell a lamport
    /// transfer from a token transfer. Empty when unknown.
    #[serde(default)]
    pub asset: String,
}

fn open_rows<'a>(
    rows: &'a [OutboundRef],
    ctx: &SignContext,
) -> Result<Vec<&'a OutboundRef>, String> {
    let batch = ctx.batch_id().ok_or("not a payout context")?;
    let list: Vec<&OutboundRef> = rows
        .iter()
        .filter(|r| r.chain == ctx.chain() && r.batch_id == Some(batch))
        .collect();
    if list.is_empty() {
        return Err(format!(
            "batch {} has no outbound on {}",
            batch,
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
/// Sweeps and consolidations need [`verify_with_pool`].
pub fn verify(
    ctx: &SignContext,
    digest: &[u8; 32],
    path: &[u32],
    vault_pubkey: &[u8],
    chain_code: &[u8; 32],
    rows: &[OutboundRef],
) -> Result<(), String> {
    verify_with_pool(ctx, digest, path, vault_pubkey, chain_code, rows, None)
}

/// [`verify`] plus the custody-vault moves: a sweep to the pool's active
/// generation (`pool`), a consolidation into a hot address, gas funding of
/// a deposit index. Those pay no outbound row; what they may pay is fixed
/// by the keys instead.
pub fn verify_with_pool(
    ctx: &SignContext,
    digest: &[u8; 32],
    path: &[u32],
    vault_pubkey: &[u8],
    chain_code: &[u8; 32],
    rows: &[OutboundRef],
    pool: Option<&PoolRef>,
) -> Result<(), String> {
    if ctx.recovery_id().is_some() {
        return Err("a recovery context is checked by verify_recovery".into());
    }
    if ctx.chain().is_ed25519() {
        return verify_solana(ctx, digest, path, vault_pubkey, chain_code, rows, pool);
    }
    if ctx.batch_id().is_some() {
        return verify_batch(ctx, digest, path, vault_pubkey, chain_code, rows);
    }
    if ctx.needs_pool() && pool.is_none() {
        return Err("sweep needs the pool's active generation".into());
    }
    let vault_pubkey = address::secp_key(vault_pubkey).map_err(|e| e.to_string())?;
    let vault_pubkey = &vault_pubkey;
    let key = hd::child_pubkey(vault_pubkey, chain_code, path)
        .map_err(|e| format!("signing path: {e}"))?
        .public_key;
    let own_hot = hd::child_pubkey(vault_pubkey, chain_code, &hd::deposit_path(ctx.chain(), 0))
        .map_err(|e| e.to_string())?
        .public_key;
    let active_hot = |p: &PoolRef| -> Result<[u8; 33], String> {
        let pk = address::secp_key(&p.active_pubkey).map_err(|e| e.to_string())?;
        Ok(
            hd::child_pubkey(&pk, &p.active_chain_code, &hd::deposit_path(ctx.chain(), 0))
                .map_err(|e| e.to_string())?
                .public_key,
        )
    };
    match ctx {
        SignContext::BtcSweep {
            raw_tx,
            input,
            prevout_value,
            prevout_pubkey,
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
            let dest = btc::p2wpkh_script(&active_hot(pool.ok_or("no pool")?)?)
                .map_err(|e| e.to_string())?;
            if tx.output.len() != 1 || tx.output[0].script_pubkey != dest {
                return Err(
                    "a sweep pays exactly one output: the active generation's own address".into(),
                );
            }
            Ok(())
        }
        SignContext::EthSweep {
            chain_id,
            nonce,
            max_priority_fee_per_gas,
            max_fee_per_gas,
            gas_limit,
            to,
            value,
            data,
            ..
        }
        | SignContext::EthConsolidate {
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
            let (dest, _amount) = eth_tx_destination(
                digest,
                *chain_id,
                *nonce,
                *max_priority_fee_per_gas,
                *max_fee_per_gas,
                *gas_limit,
                to,
                *value,
                data,
            )?;
            let expected = if ctx.needs_pool() {
                active_hot(pool.ok_or("no pool")?)?
            } else {
                own_hot
            };
            let expected =
                address::eth_decode(&address::eth_address(&expected).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if dest != expected {
                return Err("destination is not the hot address this move may pay".into());
            }
            Ok(())
        }
        SignContext::EthGasFund {
            index,
            chain_id,
            nonce,
            max_priority_fee_per_gas,
            max_fee_per_gas,
            gas_limit,
            to,
            value,
            ..
        } => {
            if key != own_hot {
                return Err("gas is funded from the hot address only".into());
            }
            if *value > ETH_GAS_FUND_CAP_WEI {
                return Err("gas funding above the cap".into());
            }
            let (dest, _) = eth_tx_destination(
                digest,
                *chain_id,
                *nonce,
                *max_priority_fee_per_gas,
                *max_fee_per_gas,
                *gas_limit,
                to,
                *value,
                "",
            )?;
            let child = hd::child_pubkey(
                vault_pubkey,
                chain_code,
                &hd::deposit_path(ctx.chain(), *index),
            )
            .map_err(|e| e.to_string())?
            .public_key;
            let expected =
                address::eth_decode(&address::eth_address(&child).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if dest != expected {
                return Err("gas funding may only pay this key's own deposit index".into());
            }
            Ok(())
        }
        SignContext::TronSweep { raw_data, .. }
        | SignContext::TronConsolidate { raw_data, .. }
        | SignContext::TronGasFund { raw_data, .. } => {
            let (owner, dest, amount, native) = tron_tx_parts(digest, raw_data)?;
            let owner_want =
                address::tron_decode(&address::tron_address(&key).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if owner.as_slice() != owner_want {
                return Err("the signing path does not derive the transaction's owner".into());
            }
            let expected_key = match ctx {
                SignContext::TronGasFund { index, .. } => {
                    if key != own_hot {
                        return Err("gas is funded from the hot address only".into());
                    }
                    if !native || amount > TRON_GAS_FUND_CAP_SUN {
                        return Err("gas funding must be a capped TRX transfer".into());
                    }
                    hd::child_pubkey(
                        vault_pubkey,
                        chain_code,
                        &hd::deposit_path(Chain::Tron, *index),
                    )
                    .map_err(|e| e.to_string())?
                    .public_key
                }
                _ if ctx.needs_pool() => active_hot(pool.ok_or("no pool")?)?,
                _ => own_hot,
            };
            let expected = address::tron_decode(
                &address::tron_address(&expected_key).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if dest != expected {
                return Err("destination is not the address this move may pay".into());
            }
            Ok(())
        }
        _ => Err("unexpected context".into()),
    }
}

/// Check an EIP-1559 description against `digest` and return the
/// transfer's (recipient, amount): the `to` of a native transfer or the
/// recipient inside an ERC-20 `transfer`.
#[allow(clippy::too_many_arguments)]
fn eth_tx_destination(
    digest: &[u8; 32],
    chain_id: u64,
    nonce: u64,
    max_priority_fee_per_gas: u128,
    max_fee_per_gas: u128,
    gas_limit: u64,
    to: &str,
    value: u128,
    data: &str,
) -> Result<([u8; 20], u128), String> {
    let to20 = address::eth_decode(to).map_err(|e| format!("to: {e}"))?;
    let data = hex::decode(data.trim_start_matches("0x")).map_err(|e| format!("data: {e}"))?;
    let tx = Eip1559Tx {
        chain_id,
        nonce,
        max_priority_fee_per_gas,
        max_fee_per_gas,
        gas_limit,
        to: to20,
        value,
        data: data.clone(),
    };
    if tx.signing_hash() != *digest {
        return Err("digest is not the signing hash of the described transaction".into());
    }
    if data.is_empty() {
        Ok((to20, value))
    } else {
        if value != 0 {
            return Err("token transfer with a native value".into());
        }
        parse_erc20_transfer(&data)
    }
}

/// Check a Tron raw transaction against `digest` and return
/// (owner, destination, amount, is_native).
fn tron_tx_parts(
    digest: &[u8; 32],
    raw_data: &str,
) -> Result<(Vec<u8>, [u8; 21], u128, bool), String> {
    let raw = hex::decode(raw_data).map_err(|e| format!("raw data: {e}"))?;
    let want: [u8; 32] = sha2::Sha256::digest_bytes(&raw);
    if want != *digest {
        return Err("digest is not the id of the described transaction".into());
    }
    let decoded = tron::Raw::decode(raw.as_slice()).map_err(|e| format!("raw data: {e}"))?;
    if decoded.encode_to_vec() != raw {
        return Err("raw data is not canonically encoded".into());
    }
    tron_transfer_parts(&decoded)
}

/// [`tron_transfer_parts`] of raw `raw_data` bytes.
pub fn tron_transfer_from_bytes(raw: &[u8]) -> Result<(Vec<u8>, [u8; 21], u128, bool), String> {
    let decoded = tron::Raw::decode(raw).map_err(|e| format!("raw data: {e}"))?;
    tron_transfer_parts(&decoded)
}

/// The single transfer a Tron raw transaction carries: (owner,
/// destination, amount, is_native); a TRC-20 `transfer` call is unpacked.
pub fn tron_transfer_parts(decoded: &tron::Raw) -> Result<(Vec<u8>, [u8; 21], u128, bool), String> {
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
    match contract.r#type {
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
            Ok((c.owner_address.clone(), dest, c.amount as u128, true))
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
            Ok((c.owner_address.clone(), dest, amount, false))
        }
        other => Err(format!("unsupported contract type {other}")),
    }
}

/// A recovery: the transaction must move coins out of the row's deposit
/// index, to the row's destination, in the row's token, and nothing else.
/// The amount is free (it is the owner's coins to the owner's address).
/// The signing path must derive the index (or, on Solana, the hot address
/// that pays the fee).
pub fn verify_recovery(
    ctx: &SignContext,
    digest: &[u8; 32],
    path: &[u32],
    vault_pubkey: &[u8],
    chain_code: &[u8; 32],
    rows: &[RecoveryRef],
) -> Result<(), String> {
    let id = ctx.recovery_id().ok_or("not a recovery context")?;
    let row = rows
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("recovery {id} is not open"))?;
    if !row.status.eq_ignore_ascii_case("Open") {
        return Err(format!("recovery {id} is {}", row.status));
    }
    let family = row.chain.address_family();
    match ctx {
        SignContext::EthRecover {
            index,
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
            if !row.chain.is_evm() {
                return Err("row is not on an EVM chain".into());
            }
            if *index != row.deposit_index || *path != hd::deposit_path(family, *index) {
                return Err("the signing path is not the row's deposit index".into());
            }
            match row.evm_chain_id {
                Some(want) if want != *chain_id => {
                    return Err(format!("chain id {chain_id} is not the row's {want}"))
                }
                Some(_) => {}
                None => {
                    if crate::evm_chain_of_id(*chain_id) != row.chain {
                        return Err(format!("chain id {chain_id} is not {}", row.chain.as_str()));
                    }
                }
            }
            let (dest, _amount) = eth_tx_destination(
                digest,
                *chain_id,
                *nonce,
                *max_priority_fee_per_gas,
                *max_fee_per_gas,
                *gas_limit,
                to,
                *value,
                data,
            )?;
            let want = address::eth_decode(&row.to).map_err(|e| format!("row.to: {e}"))?;
            if dest != want {
                return Err("destination is not the row's".into());
            }
            let to20 = address::eth_decode(to).map_err(|e| format!("to: {e}"))?;
            match &row.token {
                Some(contract) => {
                    let c = address::eth_decode(contract).map_err(|e| format!("token: {e}"))?;
                    if to20 != c || *value != 0 {
                        return Err("a token recovery calls the row's token contract".into());
                    }
                }
                None => {
                    if !data.trim_start_matches("0x").is_empty() || to20 != want {
                        return Err("a native recovery is a plain value transfer".into());
                    }
                }
            }
            Ok(())
        }
        SignContext::TronRecover {
            index, raw_data, ..
        } => {
            if row.chain != Chain::Tron {
                return Err("row is not on Tron".into());
            }
            if *index != row.deposit_index || *path != hd::deposit_path(Chain::Tron, *index) {
                return Err("the signing path is not the row's deposit index".into());
            }
            let vault = address::secp_key(vault_pubkey).map_err(|e| e.to_string())?;
            let key = hd::child_pubkey(&vault, chain_code, path)
                .map_err(|e| e.to_string())?
                .public_key;
            let (owner, dest, _amount, native) = tron_tx_parts(digest, raw_data)?;
            let owner_want =
                address::tron_decode(&address::tron_address(&key).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if owner.as_slice() != owner_want {
                return Err("the signing path does not derive the transaction's owner".into());
            }
            let want = address::tron_decode(&row.to).map_err(|e| format!("row.to: {e}"))?;
            if dest != want {
                return Err("destination is not the row's".into());
            }
            if native != row.token.is_none() {
                return Err("native/token kind does not match the row".into());
            }
            Ok(())
        }
        SignContext::SolRecover { index, message, .. } => {
            if row.chain != Chain::Solana {
                return Err("row is not on Solana".into());
            }
            if *index != row.deposit_index {
                return Err("the context names another index than the row".into());
            }
            let bytes = hex::decode(message).map_err(|e| format!("message: {e}"))?;
            if keel_crypto_sha256(&bytes) != *digest {
                return Err("digest is not the hash of the described message".into());
            }
            let msg = sol::Message::deserialize(&bytes).map_err(|e| format!("message: {e}"))?;
            let vault = address::ed25519_key(vault_pubkey).map_err(|e| e.to_string())?;
            let key = sol::child_pubkey(&vault, chain_code, path)
                .map_err(|e| format!("signing path: {e}"))?;
            let source =
                sol::child_pubkey(&vault, chain_code, &hd::deposit_path(Chain::Solana, *index))
                    .map_err(|e| e.to_string())?;
            let own_hot =
                sol::child_pubkey(&vault, chain_code, &hd::deposit_path(Chain::Solana, 0))
                    .map_err(|e| e.to_string())?;
            if key != source && key != own_hot {
                return Err(
                    "the signing path is neither the row's index nor the hot address".into(),
                );
            }
            if !msg.signers().contains(&key) {
                return Err("the signing path does not derive a signer of the message".into());
            }
            let payer = msg.fee_payer().copied().ok_or("message has no fee payer")?;
            if payer != source && payer != own_hot {
                return Err("the fee payer is neither the index nor the hot address".into());
            }
            let dest = sol::decode(&row.to).map_err(|e| format!("row.to: {e}"))?;
            let mint = match &row.token {
                Some(m) => Some(sol::decode(m).map_err(|e| format!("token: {e}"))?),
                None => None,
            };
            let transfers = msg.transfers().map_err(|e| e.to_string())?;
            if transfers.is_empty() {
                return Err("the message moves nothing".into());
            }
            for t in &transfers {
                match (t, mint) {
                    (sol::Transfer::Lamports { from, to, .. }, None) => {
                        if *from != source || *to != dest {
                            return Err(
                                "lamports must move from the index to the row's destination".into(),
                            );
                        }
                    }
                    (
                        sol::Transfer::Token {
                            source: src,
                            mint: m,
                            destination,
                            owner,
                            ..
                        },
                        Some(want_mint),
                    ) => {
                        let ata = |o: &[u8; 32]| {
                            sol::associated_token_account(o, &want_mint).map_err(|e| e.to_string())
                        };
                        if *m != want_mint
                            || *owner != source
                            || *src != ata(&source)?
                            || *destination != ata(&dest)?
                        {
                            return Err(
                                "tokens must move from the index's account to the destination's"
                                    .into(),
                            );
                        }
                    }
                    (
                        sol::Transfer::CreateAccount {
                            payer: p,
                            owner,
                            mint: m,
                        },
                        Some(want_mint),
                    ) => {
                        if *p != payer || *owner != dest || *m != want_mint {
                            return Err(
                                "only the destination's token account may be created".into()
                            );
                        }
                    }
                    _ => return Err("transfer kind does not match the row's token".into()),
                }
            }
            Ok(())
        }
        _ => Err("not a recovery context".into()),
    }
}

/// Solana: the message's transfers, read back from its bytes, must be
/// exactly what the context allows. Every message is paid by the vault's
/// hot address; a payout pays the open rows of its batch (lamports to the
/// recipient, tokens to the recipient's associated account of the asset's
/// mint), a sweep moves the hot address to the active generation's, a
/// consolidation moves a deposit index to a hot address. The signing path
/// must derive one of the message's signers.
fn verify_solana(
    ctx: &SignContext,
    digest: &[u8; 32],
    path: &[u32],
    vault_pubkey: &[u8],
    chain_code: &[u8; 32],
    rows: &[OutboundRef],
    pool: Option<&PoolRef>,
) -> Result<(), String> {
    let message = ctx.message().ok_or("not a Solana context")?;
    let bytes = hex::decode(message).map_err(|e| format!("message: {e}"))?;
    if keel_crypto_sha256(&bytes) != *digest {
        return Err("digest is not the hash of the described message".into());
    }
    let msg = sol::Message::deserialize(&bytes).map_err(|e| format!("message: {e}"))?;
    let vault = address::ed25519_key(vault_pubkey).map_err(|e| e.to_string())?;
    let key =
        sol::child_pubkey(&vault, chain_code, path).map_err(|e| format!("signing path: {e}"))?;
    let own_hot = sol::child_pubkey(&vault, chain_code, &hd::deposit_path(Chain::Solana, 0))
        .map_err(|e| e.to_string())?;
    if !msg.signers().contains(&key) {
        return Err("the signing path does not derive a signer of the message".into());
    }
    if msg.fee_payer() != Some(&own_hot) {
        return Err("the message is not paid by the vault's hot address".into());
    }
    let transfers = msg.transfers().map_err(|e| e.to_string())?;
    let ata = |owner: &[u8; 32], mint: &[u8; 32]| -> Result<[u8; 32], String> {
        sol::associated_token_account(owner, mint).map_err(|e| e.to_string())
    };
    match ctx {
        SignContext::Sol { mints, .. } => {
            let rows = open_rows(rows, ctx)?;
            let batch = ctx.batch_id().unwrap_or(0);
            let mint_of = |asset: &str| -> Result<[u8; 32], String> {
                let (_, m) = mints
                    .iter()
                    .find(|(a, _)| a == asset)
                    .ok_or_else(|| format!("no mint for {asset} in the context"))?;
                sol::decode(m).map_err(|e| format!("mint: {e}"))
            };
            let recipients: Vec<[u8; 32]> = rows
                .iter()
                .map(|r| sol::decode(&r.to).map_err(|e| format!("outbound {}: {e}", r.id)))
                .collect::<Result<_, _>>()?;
            let mut matched = vec![false; rows.len()];
            for t in &transfers {
                match t {
                    sol::Transfer::Lamports { from, to, lamports } => {
                        if *from != own_hot {
                            return Err("lamports do not leave the hot address".into());
                        }
                        let hit = rows.iter().enumerate().find(|(j, r)| {
                            !matched[*j]
                                && r.asset == "SOL.SOL"
                                && r.amount == *lamports as u128
                                && recipients[*j] == *to
                        });
                        match hit {
                            Some((j, _)) => matched[j] = true,
                            None => {
                                return Err(format!(
                                    "transfer of {lamports} lamports pays nothing in batch {batch}"
                                ))
                            }
                        }
                    }
                    sol::Transfer::Token {
                        source,
                        mint,
                        destination,
                        owner,
                        amount,
                        ..
                    } => {
                        if *owner != own_hot || *source != ata(&own_hot, mint)? {
                            return Err("tokens do not leave the hot address's account".into());
                        }
                        let mut hit = None;
                        for (j, r) in rows.iter().enumerate() {
                            if matched[j] || r.asset == "SOL.SOL" || r.amount != *amount as u128 {
                                continue;
                            }
                            if mint_of(&r.asset)? == *mint
                                && ata(&recipients[j], mint)? == *destination
                            {
                                hit = Some(j);
                                break;
                            }
                        }
                        match hit {
                            Some(j) => matched[j] = true,
                            None => {
                                return Err(format!(
                                    "token transfer of {amount} pays nothing in batch {batch}"
                                ))
                            }
                        }
                    }
                    sol::Transfer::CreateAccount { payer, owner, mint } => {
                        if *payer != own_hot {
                            return Err("account creation is paid by the hot address only".into());
                        }
                        let ok = rows.iter().enumerate().any(|(j, r)| {
                            recipients[j] == *owner
                                && r.asset != "SOL.SOL"
                                && mint_of(&r.asset).ok() == Some(*mint)
                        });
                        if !ok {
                            return Err("account created for someone the batch does not pay".into());
                        }
                    }
                }
            }
            if let Some(j) = matched.iter().position(|m| !m) {
                return Err(format!("outbound {} of the batch is not paid", rows[j].id));
            }
            Ok(())
        }
        SignContext::SolSweep { .. } | SignContext::SolConsolidate { .. } => {
            if ctx.needs_pool() && pool.is_none() {
                return Err("sweep needs the pool's active generation".into());
            }
            let target = match (ctx.needs_pool(), pool) {
                (true, Some(p)) => {
                    let active =
                        address::ed25519_key(&p.active_pubkey).map_err(|e| e.to_string())?;
                    sol::child_pubkey(
                        &active,
                        &p.active_chain_code,
                        &hd::deposit_path(Chain::Solana, 0),
                    )
                    .map_err(|e| e.to_string())?
                }
                _ => own_hot,
            };
            let source = match ctx {
                SignContext::SolConsolidate { index, .. } => {
                    sol::child_pubkey(&vault, chain_code, &hd::deposit_path(Chain::Solana, *index))
                        .map_err(|e| e.to_string())?
                }
                _ => own_hot,
            };
            if key != own_hot && key != source {
                return Err(
                    "the signing path is neither the hot address nor the moved index".into(),
                );
            }
            if transfers.is_empty() {
                return Err("the message moves nothing".into());
            }
            for t in &transfers {
                match t {
                    sol::Transfer::Lamports { from, to, .. } => {
                        if *from != source || *to != target {
                            return Err(
                                "lamports must move from the index to the hot address".into()
                            );
                        }
                    }
                    sol::Transfer::Token {
                        source: src,
                        mint,
                        destination,
                        owner,
                        ..
                    } => {
                        if *owner != source
                            || *src != ata(&source, mint)?
                            || *destination != ata(&target, mint)?
                        {
                            return Err(
                                "tokens must move from the index's account to the hot address's"
                                    .into(),
                            );
                        }
                    }
                    sol::Transfer::CreateAccount { payer, owner, .. } => {
                        if *payer != own_hot || *owner != target {
                            return Err(
                                "only the target hot address's token account may be created".into(),
                            );
                        }
                    }
                }
            }
            Ok(())
        }
        _ => Err("unexpected context".into()),
    }
}

fn keel_crypto_sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).into()
}

fn verify_batch(
    ctx: &SignContext,
    digest: &[u8; 32],
    path: &[u32],
    vault_pubkey: &[u8],
    chain_code: &[u8; 32],
    rows: &[OutboundRef],
) -> Result<(), String> {
    let rows = open_rows(rows, ctx)?;
    let batch = ctx.batch_id().unwrap_or(0);
    let vault_pubkey = address::secp_key(vault_pubkey).map_err(|e| e.to_string())?;
    let vault_pubkey = &vault_pubkey;
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
                            batch
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
                return Err(format!("transfer pays nothing in batch {batch}"));
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
                return Err(format!("transfer pays nothing in batch {batch}"));
            }
            Ok(())
        }
        _ => Err("not a payout context".into()),
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
            asset: String::new(),
        }
    }

    fn sol_row(id: u64, to: &[u8; 32], asset: &str, amount: u128, batch: u64) -> OutboundRef {
        OutboundRef {
            id,
            chain: Chain::Solana,
            to: sol::address(to),
            amount,
            status: "Batched".into(),
            batch_id: Some(batch),
            asset: asset.into(),
        }
    }

    /// Solana: a payout message is bound to its batch's rows, a
    /// consolidation to the index and the hot address, nothing else signs.
    #[test]
    fn solana_messages_are_bound_to_rows_and_hot_addresses() {
        use curve25519_dalek::scalar::Scalar;
        let x = Scalar::from_bytes_mod_order([11u8; 32]);
        let vault = sol::pubkey_of_scalar(&x);
        let cc = [5u8; 32];
        let hot_path = hd::deposit_path(Chain::Solana, 0);
        let hot = sol::child_pubkey(&vault, &cc, &hot_path).unwrap();
        let user = sol::pubkey_of_scalar(&Scalar::from_bytes_mod_order([12u8; 32]));
        let mint = sol::decode("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU").unwrap();
        let src = sol::associated_token_account(&hot, &mint).unwrap();
        let dst = sol::associated_token_account(&user, &mint).unwrap();
        let ixs = vec![
            sol::transfer_lamports(&hot, &user, 1_000),
            sol::create_associated_token_account(&hot, &user, &mint).unwrap(),
            sol::transfer_checked(&src, &mint, &dst, &hot, 2_500_000, 6),
        ];
        let msg = sol::build_message(&hot, &ixs, [8u8; 32])
            .unwrap()
            .serialize();
        let digest = keel_crypto_sha256(&msg);
        let ctx = SignContext::Sol {
            batch_id: 4,
            message: hex::encode(&msg),
            mints: vec![("SOL.USDC".into(), sol::address(&mint))],
        };
        assert_eq!(ctx.chain(), Chain::Solana);
        let rows = vec![
            sol_row(1, &user, "SOL.SOL", 1_000, 4),
            sol_row(2, &user, "SOL.USDC", 2_500_000, 4),
        ];
        verify(&ctx, &digest, &hot_path, &vault, &cc, &rows).unwrap();
        // A row the message does not pay, a wrong amount, a wrong digest.
        let mut more = rows.clone();
        more.push(sol_row(3, &user, "SOL.SOL", 7, 4));
        assert!(verify(&ctx, &digest, &hot_path, &vault, &cc, &more).is_err());
        let wrong = vec![sol_row(1, &user, "SOL.SOL", 999, 4), rows[1].clone()];
        assert!(verify(&ctx, &digest, &hot_path, &vault, &cc, &wrong).is_err());
        assert!(verify(&ctx, &[0u8; 32], &hot_path, &vault, &cc, &rows).is_err());
        // The user's own path does not sign a payout.
        let user_path = hd::deposit_path(Chain::Solana, 3);
        assert!(verify(&ctx, &digest, &user_path, &vault, &cc, &rows).is_err());
        // A 33-byte key is not a Solana vault.
        assert!(verify(&ctx, &digest, &hot_path, &VAULT, &cc, &rows).is_err());

        // Consolidation of index 3 into the hot address, paid by the hot
        // address: both paths sign; the lamports go to the hot address.
        let idx = sol::child_pubkey(&vault, &cc, &user_path).unwrap();
        let cmsg = sol::build_message(&hot, &[sol::transfer_lamports(&idx, &hot, 500)], [8u8; 32])
            .unwrap()
            .serialize();
        let cdigest = keel_crypto_sha256(&cmsg);
        let cctx = SignContext::SolConsolidate {
            custodian: "c".into(),
            index: 3,
            sweep: false,
            message: hex::encode(&cmsg),
        };
        verify_with_pool(&cctx, &cdigest, &user_path, &vault, &cc, &[], None).unwrap();
        verify_with_pool(&cctx, &cdigest, &hot_path, &vault, &cc, &[], None).unwrap();
        // The same message under the wrong index is refused.
        let wrong_ctx = SignContext::SolConsolidate {
            custodian: "c".into(),
            index: 4,
            sweep: false,
            message: hex::encode(&cmsg),
        };
        assert!(
            verify_with_pool(&wrong_ctx, &cdigest, &user_path, &vault, &cc, &[], None).is_err()
        );
        // Paying anyone but the hot address is refused.
        let bad = sol::build_message(&hot, &[sol::transfer_lamports(&idx, &user, 500)], [8u8; 32])
            .unwrap()
            .serialize();
        let bctx = SignContext::SolConsolidate {
            custodian: "c".into(),
            index: 3,
            sweep: false,
            message: hex::encode(&bad),
        };
        assert!(verify_with_pool(
            &bctx,
            &keel_crypto_sha256(&bad),
            &user_path,
            &vault,
            &cc,
            &[],
            None
        )
        .is_err());
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

    /// The same EVM context pays a BSC batch when its chain id is BSC's:
    /// the outbound rows it may pay are the BSC ones, never Ethereum's.
    #[test]
    fn bsc_payout_is_read_from_the_chain_id() {
        let to = child(Chain::Bsc, 4);
        let to_addr = address::eth_address(&to).unwrap();
        let to20 = address::eth_decode(&to_addr).unwrap();
        let tx = Eip1559Tx::native_transfer(97, 3, to20, 1_000, 30, 2);
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
        assert_eq!(ctx.chain(), Chain::Bsc);
        // Same key path as Ethereum: the hot address is one address.
        let path = hd::deposit_path(Chain::Bsc, 0);
        assert_eq!(path, hd::deposit_path(Chain::Ethereum, 0));
        let rows = vec![row(1, Chain::Bsc, &to_addr, 1_000, 2)];
        verify(&ctx, &tx.signing_hash(), &path, &VAULT, &CC, &rows).unwrap();
        let eth_rows = vec![row(1, Chain::Ethereum, &to_addr, 1_000, 2)];
        assert!(verify(&ctx, &tx.signing_hash(), &path, &VAULT, &CC, &eth_rows).is_err());
        assert_eq!(crate::evm_chain_of_id(56), Chain::Bsc);
        assert_eq!(crate::evm_chain_of_id(1), Chain::Ethereum);
        assert_eq!(crate::evm_chain_of_id(11_155_111), Chain::Ethereum);
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

    /// The active generation of the pool in the sweep tests: another key.
    const ACTIVE: [u8; 33] = [
        0x03, 0x1b, 0x84, 0xc5, 0x56, 0x7b, 0x12, 0x64, 0x40, 0x99, 0x5d, 0x3e, 0xd5, 0xaa, 0xba,
        0x05, 0x65, 0xd7, 0x1e, 0x18, 0x34, 0x60, 0x48, 0x19, 0xff, 0x9c, 0x17, 0xf5, 0xe9, 0xd5,
        0xdd, 0x07, 0x8f,
    ];
    const ACC: [u8; 32] = [4u8; 32];

    fn pool() -> PoolRef {
        PoolRef {
            active_pubkey: ACTIVE.to_vec(),
            active_chain_code: ACC,
        }
    }

    fn active_hot(chain: Chain) -> [u8; 33] {
        hd::child_pubkey(&ACTIVE, &ACC, &hd::deposit_path(chain, 0))
            .unwrap()
            .public_key
    }

    #[test]
    fn bitcoin_sweep_pays_only_the_active_generation() {
        let net = Network::Regtest;
        let spent_key = child(Chain::Bitcoin, 5);
        let path = hd::deposit_path(Chain::Bitcoin, 5);
        let mk = |dest_key: [u8; 33], extra: bool| {
            let mut output = vec![TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: btc::p2wpkh_script(&dest_key).unwrap(),
            }];
            if extra {
                output.push(TxOut {
                    value: Amount::from_sat(1),
                    script_pubkey: btc::p2wpkh_script(&child(Chain::Bitcoin, 0)).unwrap(),
                });
            }
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
                output,
            };
            let spk = btc::p2wpkh_script(&spent_key).unwrap();
            let digest = SighashCache::new(&tx)
                .p2wpkh_signature_hash(0, &spk, Amount::from_sat(100_000), EcdsaSighashType::All)
                .unwrap()
                .to_byte_array();
            let ctx = SignContext::BtcSweep {
                custodian: "00".repeat(32),
                from_gen: 0,
                raw_tx: hex::encode(consensus::serialize(&tx)),
                input: 0,
                prevout_value: 100_000,
                prevout_pubkey: hex::encode(spent_key),
                network: net,
            };
            (ctx, digest)
        };
        let (ctx, digest) = mk(active_hot(Chain::Bitcoin), false);
        verify_with_pool(&ctx, &digest, &path, &VAULT, &CC, &[], Some(&pool())).unwrap();
        // Without the pool the signer cannot know where a sweep may go.
        assert!(verify(&ctx, &digest, &path, &VAULT, &CC, &[]).is_err());
        // To this key's own hot address: not a sweep.
        let (ctx, digest) = mk(child(Chain::Bitcoin, 0), false);
        assert!(verify_with_pool(&ctx, &digest, &path, &VAULT, &CC, &[], Some(&pool())).is_err());
        // A second output, even a tiny one back to ourselves.
        let (ctx, digest) = mk(active_hot(Chain::Bitcoin), true);
        assert!(verify_with_pool(&ctx, &digest, &path, &VAULT, &CC, &[], Some(&pool())).is_err());
    }

    #[test]
    fn ethereum_moves_are_bound_to_hot_addresses_and_gas_is_capped() {
        let eth20 = |k: [u8; 33]| address::eth_decode(&address::eth_address(&k).unwrap()).unwrap();
        let ctx_of = |tx: &Eip1559Tx, kind: &str| {
            let common = (
                tx.chain_id,
                tx.nonce,
                tx.max_priority_fee_per_gas,
                tx.max_fee_per_gas,
                tx.gas_limit,
                format!("0x{}", hex::encode(tx.to)),
                tx.value,
                hex::encode(&tx.data),
            );
            match kind {
                "sweep" => SignContext::EthSweep {
                    custodian: "00".repeat(32),
                    from_gen: 0,
                    chain_id: common.0,
                    nonce: common.1,
                    max_priority_fee_per_gas: common.2,
                    max_fee_per_gas: common.3,
                    gas_limit: common.4,
                    to: common.5,
                    value: common.6,
                    data: common.7,
                },
                "consolidate" => SignContext::EthConsolidate {
                    custodian: "00".repeat(32),
                    index: 4,
                    sweep: false,
                    chain_id: common.0,
                    nonce: common.1,
                    max_priority_fee_per_gas: common.2,
                    max_fee_per_gas: common.3,
                    gas_limit: common.4,
                    to: common.5,
                    value: common.6,
                    data: common.7,
                },
                _ => SignContext::EthGasFund {
                    custodian: "00".repeat(32),
                    index: 4,
                    chain_id: common.0,
                    nonce: common.1,
                    max_priority_fee_per_gas: common.2,
                    max_fee_per_gas: common.3,
                    gas_limit: common.4,
                    to: common.5,
                    value: common.6,
                },
            }
        };
        let hot_path = hd::deposit_path(Chain::Ethereum, 0);
        // Sweep: tokens from our hot address to the active hot address.
        let tx = Eip1559Tx::erc20_transfer(
            1,
            1,
            [0x11u8; 20],
            eth20(active_hot(Chain::Ethereum)),
            5,
            30,
            2,
        );
        let ctx = ctx_of(&tx, "sweep");
        verify_with_pool(
            &ctx,
            &tx.signing_hash(),
            &hot_path,
            &VAULT,
            &CC,
            &[],
            Some(&pool()),
        )
        .unwrap();
        let bad = Eip1559Tx::erc20_transfer(
            1,
            1,
            [0x11u8; 20],
            eth20(child(Chain::Ethereum, 7)),
            5,
            30,
            2,
        );
        let ctx = ctx_of(&bad, "sweep");
        assert!(verify_with_pool(
            &ctx,
            &bad.signing_hash(),
            &hot_path,
            &VAULT,
            &CC,
            &[],
            Some(&pool())
        )
        .is_err());
        // Consolidation: index 4 to our own hot address, and nowhere else.
        let tx = Eip1559Tx::erc20_transfer(
            1,
            2,
            [0x11u8; 20],
            eth20(child(Chain::Ethereum, 0)),
            5,
            30,
            2,
        );
        let ctx = ctx_of(&tx, "consolidate");
        let path4 = hd::deposit_path(Chain::Ethereum, 4);
        verify(&ctx, &tx.signing_hash(), &path4, &VAULT, &CC, &[]).unwrap();
        let bad = Eip1559Tx::erc20_transfer(
            1,
            2,
            [0x11u8; 20],
            eth20(active_hot(Chain::Ethereum)),
            5,
            30,
            2,
        );
        let ctx = ctx_of(&bad, "consolidate");
        assert!(verify(&ctx, &bad.signing_hash(), &path4, &VAULT, &CC, &[]).is_err());
        // Gas funding: from the hot address to our own index 4, capped.
        let tx = Eip1559Tx::native_transfer(1, 3, eth20(child(Chain::Ethereum, 4)), 1_000, 30, 2);
        let ctx = ctx_of(&tx, "gas");
        verify(&ctx, &tx.signing_hash(), &hot_path, &VAULT, &CC, &[]).unwrap();
        assert!(
            verify(&ctx, &tx.signing_hash(), &path4, &VAULT, &CC, &[]).is_err(),
            "not from hot"
        );
        let big = Eip1559Tx::native_transfer(
            1,
            3,
            eth20(child(Chain::Ethereum, 4)),
            ETH_GAS_FUND_CAP_WEI + 1,
            30,
            2,
        );
        let ctx = ctx_of(&big, "gas");
        assert!(verify(&ctx, &big.signing_hash(), &hot_path, &VAULT, &CC, &[]).is_err());
        let stranger =
            Eip1559Tx::native_transfer(1, 3, eth20(active_hot(Chain::Ethereum)), 1_000, 30, 2);
        let ctx = ctx_of(&stranger, "gas");
        assert!(verify(&ctx, &stranger.signing_hash(), &hot_path, &VAULT, &CC, &[]).is_err());
    }

    #[test]
    fn tron_sweep_and_gas_funding_are_bound() {
        let b = TronTxBuilder {
            ref_block_bytes: [1, 2],
            ref_block_hash: [3u8; 8],
            timestamp_ms: 1_000,
            expiration_ms: 61_000,
            fee_limit_sun: 100,
        };
        let t21 = |k: [u8; 33]| address::tron_decode(&address::tron_address(&k).unwrap()).unwrap();
        let owner = t21(child(Chain::Tron, 0));
        let hot_path = hd::deposit_path(Chain::Tron, 0);
        let raw = b.transfer(&owner, &t21(active_hot(Chain::Tron)), 7_000_000);
        let ctx = SignContext::TronSweep {
            custodian: "00".repeat(32),
            from_gen: 0,
            raw_data: hex::encode(raw.to_bytes()),
        };
        verify_with_pool(
            &ctx,
            &raw.txid(),
            &hot_path,
            &VAULT,
            &CC,
            &[],
            Some(&pool()),
        )
        .unwrap();
        assert!(
            verify(&ctx, &raw.txid(), &hot_path, &VAULT, &CC, &[]).is_err(),
            "no pool"
        );
        let raw = b.transfer(&owner, &t21(child(Chain::Tron, 9)), 7_000_000);
        let ctx = SignContext::TronSweep {
            custodian: "00".repeat(32),
            from_gen: 0,
            raw_data: hex::encode(raw.to_bytes()),
        };
        assert!(verify_with_pool(
            &ctx,
            &raw.txid(),
            &hot_path,
            &VAULT,
            &CC,
            &[],
            Some(&pool())
        )
        .is_err());
        // Gas: hot to our own index 3, within the cap; a TRC-20 is not gas.
        let raw = b.transfer(&owner, &t21(child(Chain::Tron, 3)), 1_000_000);
        let ctx = SignContext::TronGasFund {
            custodian: "00".repeat(32),
            index: 3,
            raw_data: hex::encode(raw.to_bytes()),
        };
        verify(&ctx, &raw.txid(), &hot_path, &VAULT, &CC, &[]).unwrap();
        let raw = b.transfer(
            &owner,
            &t21(child(Chain::Tron, 3)),
            TRON_GAS_FUND_CAP_SUN as u64 + 1,
        );
        let ctx = SignContext::TronGasFund {
            custodian: "00".repeat(32),
            index: 3,
            raw_data: hex::encode(raw.to_bytes()),
        };
        assert!(verify(&ctx, &raw.txid(), &hot_path, &VAULT, &CC, &[]).is_err());
        // Consolidation: index 3 into our own hot, signed by index 3.
        let owner3 = t21(child(Chain::Tron, 3));
        let token = address::tron_decode("TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf").unwrap();
        let raw = b.trc20_transfer(&owner3, &token, &t21(child(Chain::Tron, 0)), 2_000_000);
        let ctx = SignContext::TronConsolidate {
            custodian: "00".repeat(32),
            index: 3,
            sweep: false,
            raw_data: hex::encode(raw.to_bytes()),
        };
        verify(
            &ctx,
            &raw.txid(),
            &hd::deposit_path(Chain::Tron, 3),
            &VAULT,
            &CC,
            &[],
        )
        .unwrap();
        assert!(
            verify(&ctx, &raw.txid(), &hot_path, &VAULT, &CC, &[]).is_err(),
            "wrong owner"
        );
    }
}
