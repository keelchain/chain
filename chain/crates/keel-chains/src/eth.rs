//! Ethereum: EIP-1559 transactions, minimal RLP, ERC-20 transfer calldata,
//! signature assembly and recovery-id resolution.

use crate::address::keccak256;
use crate::Error;
use bitcoin::secp256k1::{
    ecdsa::{RecoverableSignature, RecoveryId},
    Message, PublicKey, Secp256k1,
};

// ---- minimal RLP -------------------------------------------------------

pub enum Rlp {
    Bytes(Vec<u8>),
    List(Vec<Rlp>),
}

impl Rlp {
    pub fn uint(v: u128) -> Rlp {
        if v == 0 {
            return Rlp::Bytes(Vec::new());
        }
        let bytes = v.to_be_bytes();
        let start = bytes.iter().position(|b| *b != 0).unwrap_or(16);
        Rlp::Bytes(bytes[start..].to_vec())
    }

    pub fn bytes(b: &[u8]) -> Rlp {
        Rlp::Bytes(b.to_vec())
    }

    pub fn encode(&self) -> Vec<u8> {
        match self {
            Rlp::Bytes(b) => {
                if b.len() == 1 && b[0] < 0x80 {
                    b.clone()
                } else {
                    let mut out = length_prefix(b.len(), 0x80);
                    out.extend_from_slice(b);
                    out
                }
            }
            Rlp::List(items) => {
                let body: Vec<u8> = items.iter().flat_map(|i| i.encode()).collect();
                let mut out = length_prefix(body.len(), 0xc0);
                out.extend_from_slice(&body);
                out
            }
        }
    }
}

fn length_prefix(len: usize, offset: u8) -> Vec<u8> {
    if len < 56 {
        vec![offset + len as u8]
    } else {
        let be = (len as u64).to_be_bytes();
        let start = be.iter().position(|b| *b != 0).unwrap_or(7);
        let len_bytes = &be[start..];
        let mut out = vec![offset + 55 + len_bytes.len() as u8];
        out.extend_from_slice(len_bytes);
        out
    }
}

/// ---- EIP-1559 --------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eip1559Tx {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub to: [u8; 20],
    pub value: u128,
    pub data: Vec<u8>,
}

impl Eip1559Tx {
    pub fn native_transfer(
        chain_id: u64,
        nonce: u64,
        to: [u8; 20],
        value_wei: u128,
        max_fee: u128,
        priority: u128,
    ) -> Self {
        Self {
            chain_id,
            nonce,
            max_priority_fee_per_gas: priority,
            max_fee_per_gas: max_fee,
            gas_limit: 21_000,
            to,
            value: value_wei,
            data: Vec::new(),
        }
    }

    pub fn erc20_transfer(
        chain_id: u64,
        nonce: u64,
        token: [u8; 20],
        to: [u8; 20],
        amount: u128,
        max_fee: u128,
        priority: u128,
    ) -> Self {
        Self {
            chain_id,
            nonce,
            max_priority_fee_per_gas: priority,
            max_fee_per_gas: max_fee,
            gas_limit: 90_000,
            to: token,
            value: 0,
            data: erc20_transfer_data(&to, amount),
        }
    }

    fn fields(&self) -> Vec<Rlp> {
        vec![
            Rlp::uint(self.chain_id as u128),
            Rlp::uint(self.nonce as u128),
            Rlp::uint(self.max_priority_fee_per_gas),
            Rlp::uint(self.max_fee_per_gas),
            Rlp::uint(self.gas_limit as u128),
            Rlp::bytes(&self.to),
            Rlp::uint(self.value),
            Rlp::bytes(&self.data),
            Rlp::List(Vec::new()),
        ]
    }

    /// The digest the TSS signs: keccak(0x02 || rlp(unsigned fields)).
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut payload = vec![0x02u8];
        payload.extend(Rlp::List(self.fields()).encode());
        keccak256(&payload)
    }

    /// Serialized signed transaction for `eth_sendRawTransaction`.
    pub fn raw_signed(&self, sig64: &[u8; 64], y_parity: u8) -> Vec<u8> {
        let mut f = self.fields();
        f.push(Rlp::uint(y_parity as u128));
        f.push(Rlp::Bytes(strip_leading_zeros(&sig64[..32])));
        f.push(Rlp::Bytes(strip_leading_zeros(&sig64[32..])));
        let mut out = vec![0x02u8];
        out.extend(Rlp::List(f).encode());
        out
    }

    /// Maximum wei this tx can cost in gas.
    pub fn max_gas_cost(&self) -> u128 {
        self.max_fee_per_gas.saturating_mul(self.gas_limit as u128)
    }
}

fn strip_leading_zeros(b: &[u8]) -> Vec<u8> {
    let start = b.iter().position(|x| *x != 0).unwrap_or(b.len());
    b[start..].to_vec()
}

pub fn tx_hash(raw: &[u8]) -> [u8; 32] {
    keccak256(raw)
}

/// `transfer(address,uint256)` calldata.
pub fn erc20_transfer_data(to: &[u8; 20], amount: u128) -> Vec<u8> {
    let mut out = Vec::with_capacity(68);
    out.extend_from_slice(&[0xa9, 0x05, 0x9c, 0xbb]);
    out.extend_from_slice(&[0u8; 12]);
    out.extend_from_slice(to);
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&amount.to_be_bytes());
    out
}

/// Decode an ERC-20 `Transfer` log's `to` and `amount`.
pub fn parse_transfer_log(topics: &[[u8; 32]], data: &[u8]) -> Option<([u8; 20], [u8; 20], u128)> {
    if topics.len() != 3 || topics[0] != transfer_topic() || data.len() < 32 {
        return None;
    }
    let mut from = [0u8; 20];
    from.copy_from_slice(&topics[1][12..]);
    let mut to = [0u8; 20];
    to.copy_from_slice(&topics[2][12..]);
    if data[..16].iter().any(|b| *b != 0) {
        return None; // amount above u128
    }
    let mut amt = [0u8; 16];
    amt.copy_from_slice(&data[16..32]);
    Some((from, to, u128::from_be_bytes(amt)))
}

pub fn transfer_topic() -> [u8; 32] {
    keccak256(b"Transfer(address,address,uint256)")
}

/// Which recovery id (0/1) makes `sig64` over `digest` recover `expected`.
pub fn recovery_id(digest: &[u8; 32], sig64: &[u8; 64], expected: &[u8; 33]) -> Result<u8, Error> {
    let secp = Secp256k1::verification_only();
    let msg = Message::from_digest(*digest);
    let expected = PublicKey::from_slice(expected).map_err(|e| Error::Key(e.to_string()))?;
    for v in 0..2i32 {
        let rid = RecoveryId::from_i32(v).map_err(|e| Error::Tx(e.to_string()))?;
        if let Ok(rs) = RecoverableSignature::from_compact(sig64, rid) {
            if let Ok(pk) = secp.recover_ecdsa(&msg, &rs) {
                if pk == expected {
                    return Ok(v as u8);
                }
            }
        }
    }
    Err(Error::Tx(
        "signature does not recover the expected key".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::secp256k1::SecretKey;

    #[test]
    fn rlp_vectors() {
        assert_eq!(Rlp::bytes(b"dog").encode(), vec![0x83, b'd', b'o', b'g']);
        assert_eq!(
            Rlp::List(vec![Rlp::bytes(b"cat"), Rlp::bytes(b"dog")]).encode(),
            vec![0xc8, 0x83, b'c', b'a', b't', 0x83, b'd', b'o', b'g']
        );
        assert_eq!(Rlp::bytes(b"").encode(), vec![0x80]);
        assert_eq!(Rlp::uint(0).encode(), vec![0x80]);
        assert_eq!(Rlp::uint(15).encode(), vec![0x0f]);
        assert_eq!(Rlp::uint(1024).encode(), vec![0x82, 0x04, 0x00]);
        assert_eq!(Rlp::List(vec![]).encode(), vec![0xc0]);
        let long = vec![b'a'; 56];
        let enc = Rlp::bytes(&long).encode();
        assert_eq!(&enc[..2], &[0xb8, 56]);
    }

    #[test]
    fn erc20_selector_and_log_parsing() {
        let d = erc20_transfer_data(&[0x11; 20], 5);
        assert_eq!(&d[..4], &[0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(d.len(), 68);
        assert_eq!(
            hex::encode(transfer_topic()),
            "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"
        );
        let mut t1 = [0u8; 32];
        t1[12..].copy_from_slice(&[0x22; 20]);
        let mut t2 = [0u8; 32];
        t2[12..].copy_from_slice(&[0x33; 20]);
        let mut data = [0u8; 32];
        data[31] = 9;
        let (from, to, amt) = parse_transfer_log(&[transfer_topic(), t1, t2], &data).unwrap();
        assert_eq!(from, [0x22; 20]);
        assert_eq!(to, [0x33; 20]);
        assert_eq!(amt, 9);
    }

    #[test]
    fn signed_tx_recovers_signer_and_hashes() {
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&[7u8; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&secp, &sk).serialize();
        let tx = Eip1559Tx::erc20_transfer(
            11155111,
            3,
            [0xaa; 20],
            [0xbb; 20],
            1_000_000,
            30_000_000_000,
            1_000_000_000,
        );
        let digest = tx.signing_hash();
        let sig = secp.sign_ecdsa(&Message::from_digest(digest), &sk);
        let sig64 = sig.serialize_compact();
        let v = recovery_id(&digest, &sig64, &pk).unwrap();
        let raw = tx.raw_signed(&sig64, v);
        assert_eq!(raw[0], 0x02);
        assert_eq!(tx_hash(&raw).len(), 32);
        // A different key does not recover.
        let other = PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&[8u8; 32]).unwrap())
            .serialize();
        assert!(recovery_id(&digest, &sig64, &other).is_err());
        assert_eq!(tx.max_gas_cost(), 90_000 * 30_000_000_000);
    }
}
