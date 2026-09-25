//! Tron: TransferContract (TRX) and TriggerSmartContract (TRC-20 transfer)
//! transactions as canonical protobuf, txid = sha256(raw_data), signature
//! = r || s || (recid + 27).

use crate::address::keccak256;
use prost::Message;
use sha2::{Digest as _, Sha256};

pub const TYPE_URL_TRANSFER: &str = "type.googleapis.com/protocol.TransferContract";
pub const TYPE_URL_TRIGGER: &str = "type.googleapis.com/protocol.TriggerSmartContract";
pub const CONTRACT_TYPE_TRANSFER: i32 = 1;
pub const CONTRACT_TYPE_TRIGGER: i32 = 31;

#[derive(Clone, PartialEq, Message)]
pub struct TransferContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub to_address: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub amount: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct TriggerSmartContract {
    #[prost(bytes = "vec", tag = "1")]
    pub owner_address: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub contract_address: Vec<u8>,
    #[prost(int64, tag = "3")]
    pub call_value: i64,
    #[prost(bytes = "vec", tag = "4")]
    pub data: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Any {
    #[prost(string, tag = "1")]
    pub type_url: String,
    #[prost(bytes = "vec", tag = "2")]
    pub value: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Contract {
    #[prost(int32, tag = "1")]
    pub r#type: i32,
    #[prost(message, optional, tag = "2")]
    pub parameter: Option<Any>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Raw {
    #[prost(bytes = "vec", tag = "1")]
    pub ref_block_bytes: Vec<u8>,
    #[prost(bytes = "vec", tag = "4")]
    pub ref_block_hash: Vec<u8>,
    #[prost(int64, tag = "8")]
    pub expiration: i64,
    #[prost(message, repeated, tag = "11")]
    pub contract: Vec<Contract>,
    #[prost(int64, tag = "14")]
    pub timestamp: i64,
    #[prost(int64, tag = "18")]
    pub fee_limit: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct Transaction {
    #[prost(message, optional, tag = "1")]
    pub raw_data: Option<Raw>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub signature: Vec<Vec<u8>>,
}

/// Reference block fields from `/wallet/getnowblock` or `getblockbynum`:
/// `ref_block_bytes` = block number bytes 6..8 (big-endian u64), and
/// `ref_block_hash` = block id bytes 8..16.
pub fn ref_block(block_number: u64, block_id: &[u8; 32]) -> ([u8; 2], [u8; 8]) {
    let num = block_number.to_be_bytes();
    let mut rb = [0u8; 2];
    rb.copy_from_slice(&num[6..8]);
    let mut rh = [0u8; 8];
    rh.copy_from_slice(&block_id[8..16]);
    (rb, rh)
}

pub struct TronTxBuilder {
    pub ref_block_bytes: [u8; 2],
    pub ref_block_hash: [u8; 8],
    /// Milliseconds; expiration is typically now + 60s.
    pub timestamp_ms: u64,
    pub expiration_ms: u64,
    /// Sun; only meaningful for smart-contract calls.
    pub fee_limit_sun: u64,
}

impl TronTxBuilder {
    fn raw(&self, contract: Contract, with_fee_limit: bool) -> Raw {
        Raw {
            ref_block_bytes: self.ref_block_bytes.to_vec(),
            ref_block_hash: self.ref_block_hash.to_vec(),
            expiration: self.expiration_ms as i64,
            contract: vec![contract],
            timestamp: self.timestamp_ms as i64,
            fee_limit: if with_fee_limit {
                self.fee_limit_sun as i64
            } else {
                0
            },
        }
    }

    /// Native TRX transfer. Addresses are 21-byte (0x41-prefixed).
    pub fn transfer(&self, owner: &[u8; 21], to: &[u8; 21], amount_sun: u64) -> Raw {
        let c = TransferContract {
            owner_address: owner.to_vec(),
            to_address: to.to_vec(),
            amount: amount_sun as i64,
        };
        self.raw(
            Contract {
                r#type: CONTRACT_TYPE_TRANSFER,
                parameter: Some(Any {
                    type_url: TYPE_URL_TRANSFER.into(),
                    value: c.encode_to_vec(),
                }),
            },
            false,
        )
    }

    /// TRC-20 `transfer(address,uint256)`.
    pub fn trc20_transfer(
        &self,
        owner: &[u8; 21],
        token: &[u8; 21],
        to: &[u8; 21],
        amount: u128,
    ) -> Raw {
        let mut data = Vec::with_capacity(68);
        data.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
        data.extend_from_slice(&[0u8; 12]);
        data.extend_from_slice(&to[1..]);
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&amount.to_be_bytes());
        let c = TriggerSmartContract {
            owner_address: owner.to_vec(),
            contract_address: token.to_vec(),
            call_value: 0,
            data,
        };
        self.raw(
            Contract {
                r#type: CONTRACT_TYPE_TRIGGER,
                parameter: Some(Any {
                    type_url: TYPE_URL_TRIGGER.into(),
                    value: c.encode_to_vec(),
                }),
            },
            true,
        )
    }
}

impl Raw {
    pub fn to_bytes(&self) -> Vec<u8> {
        self.encode_to_vec()
    }

    /// The transaction id and the digest the TSS signs.
    pub fn txid(&self) -> [u8; 32] {
        Sha256::digest(self.to_bytes()).into()
    }

    /// 65-byte signature: r || s || (recid + 27), as java-tron expects.
    pub fn signature(sig64: &[u8; 64], recid: u8) -> Vec<u8> {
        let mut s = sig64.to_vec();
        s.push(recid + 27);
        s
    }

    /// Full signed transaction protobuf, hex-encoded for
    /// `POST /wallet/broadcasthex {"transaction": "<hex>"}`.
    pub fn signed_hex(&self, sig65: Vec<u8>) -> String {
        let tx = Transaction {
            raw_data: Some(self.clone()),
            signature: vec![sig65],
        };
        hex::encode(tx.encode_to_vec())
    }

    /// JSON for `POST /wallet/broadcasttransaction`.
    pub fn broadcast_json(&self, sig65: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "txID": hex::encode(self.txid()),
            "raw_data_hex": hex::encode(self.to_bytes()),
            "signature": [hex::encode(sig65)],
        })
    }
}

/// The 4-byte selector of a Solidity function signature.
pub fn selector(signature: &str) -> [u8; 4] {
    let h = keccak256(signature.as_bytes());
    [h[0], h[1], h[2], h[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builder() -> TronTxBuilder {
        TronTxBuilder {
            ref_block_bytes: [0x12, 0x34],
            ref_block_hash: [1, 2, 3, 4, 5, 6, 7, 8],
            timestamp_ms: 1_700_000_000_000,
            expiration_ms: 1_700_000_060_000,
            fee_limit_sun: 100_000_000,
        }
    }

    #[test]
    fn transfer_encodes_canonically_and_round_trips() {
        let owner = [0x41u8; 21];
        let to = [0x42u8; 21];
        let raw = builder().transfer(&owner, &to, 1_000_000);
        let bytes = raw.to_bytes();
        let decoded = Raw::decode(bytes.as_slice()).unwrap();
        assert_eq!(decoded, raw);
        // Default fee_limit (0) is omitted from the encoding.
        assert!(!bytes.windows(2).any(|w| w == [0x90, 0x01]));
        let inner = TransferContract::decode(
            decoded.contract[0]
                .parameter
                .as_ref()
                .unwrap()
                .value
                .as_slice(),
        )
        .unwrap();
        assert_eq!(inner.amount, 1_000_000);
        assert_eq!(raw.txid().len(), 32);
        let sig65 = Raw::signature(&[9u8; 64], 1);
        assert_eq!(sig65.len(), 65);
        assert_eq!(sig65[64], 28);
        let tx = Transaction::decode(
            hex::decode(raw.signed_hex(sig65.clone()))
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        assert_eq!(tx.signature[0], sig65);
        let j = raw.broadcast_json(&sig65);
        assert_eq!(j["txID"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn trc20_transfer_calldata_and_fee_limit() {
        let raw = builder().trc20_transfer(&[0x41; 21], &[0x43; 21], &[0x44; 21], 5_000_000);
        assert_eq!(raw.fee_limit, 100_000_000);
        let inner = TriggerSmartContract::decode(
            raw.contract[0].parameter.as_ref().unwrap().value.as_slice(),
        )
        .unwrap();
        assert_eq!(&inner.data[..4], &selector("transfer(address,uint256)"));
        assert_eq!(inner.data.len(), 68);
        assert_eq!(&inner.data[16..36], &[0x44; 20]);
        assert_eq!(raw.contract[0].r#type, CONTRACT_TYPE_TRIGGER);
    }

    #[test]
    fn ref_block_from_number_and_id() {
        let mut id = [0u8; 32];
        id[8..16].copy_from_slice(&[0xaa; 8]);
        let (rb, rh) = ref_block(0x0001_0203_0405_0607, &id);
        assert_eq!(rb, [0x06, 0x07]);
        assert_eq!(rh, [0xaa; 8]);
    }
}
