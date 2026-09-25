//! Address encoders and validators per chain.

use crate::{hd, Error, Network};
use bitcoin::{Address as BtcAddress, CompressedPublicKey};
use keel_actions::Chain;
use std::str::FromStr;
use tiny_keccak::{Hasher as _, Keccak};

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// 20-byte EVM-style account hash of a secp256k1 public key.
pub fn evm_account(pubkey33: &[u8; 33]) -> Result<[u8; 20], Error> {
    let pk = bitcoin::secp256k1::PublicKey::from_slice(pubkey33)
        .map_err(|e| Error::Key(e.to_string()))?;
    let uncompressed = pk.serialize_uncompressed();
    let hash = keccak256(&uncompressed[1..]);
    let mut out = [0u8; 20];
    out.copy_from_slice(&hash[12..]);
    Ok(out)
}

/// EIP-55 checksummed hex.
pub fn eth_checksum(account: &[u8; 20]) -> String {
    let lower = hex::encode(account);
    let hash = keccak256(lower.as_bytes());
    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, c) in lower.chars().enumerate() {
        let nibble = (hash[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0xf;
        if c.is_ascii_alphabetic() && nibble >= 8 {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

pub fn eth_address(pubkey33: &[u8; 33]) -> Result<String, Error> {
    Ok(eth_checksum(&evm_account(pubkey33)?))
}

/// Tron base58check address: 0x41 || account hash, double-sha256 checksum.
pub fn tron_address_from_account(account: &[u8; 20]) -> String {
    let mut raw = Vec::with_capacity(21);
    raw.push(0x41);
    raw.extend_from_slice(account);
    bs58::encode(raw).with_check().into_string()
}

pub fn tron_address(pubkey33: &[u8; 33]) -> Result<String, Error> {
    Ok(tron_address_from_account(&evm_account(pubkey33)?))
}

/// Decode a Tron base58check address into its 21-byte (0x41-prefixed) form.
pub fn tron_decode(addr: &str) -> Result<[u8; 21], Error> {
    let bytes = bs58::decode(addr)
        .with_check(None)
        .into_vec()
        .map_err(|e| Error::Address(e.to_string()))?;
    if bytes.len() != 21 || bytes[0] != 0x41 {
        return Err(Error::Address(
            "tron address must be 21 bytes with 0x41 prefix".into(),
        ));
    }
    let mut out = [0u8; 21];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Decode a hex EVM address (with or without 0x, checksum not enforced).
pub fn eth_decode(addr: &str) -> Result<[u8; 20], Error> {
    let s = addr.strip_prefix("0x").unwrap_or(addr);
    let bytes = hex::decode(s).map_err(|e| Error::Address(e.to_string()))?;
    let arr: [u8; 20] = bytes
        .try_into()
        .map_err(|_| Error::Address("eth address must be 20 bytes".into()))?;
    // Reject wrong-case checksums when the input is mixed case.
    let has_upper = s.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = s.chars().any(|c| c.is_ascii_lowercase());
    if has_upper && has_lower && eth_checksum(&arr) != format!("0x{s}") {
        return Err(Error::Address("bad EIP-55 checksum".into()));
    }
    Ok(arr)
}

pub fn btc_address(pubkey33: &[u8; 33], network: Network) -> Result<String, Error> {
    let pk = CompressedPublicKey::from_slice(pubkey33).map_err(|e| Error::Key(e.to_string()))?;
    Ok(BtcAddress::p2wpkh(&pk, network.bitcoin()).to_string())
}

/// The deposit address of index `index` for `chain` under a vault key.
pub fn deposit_address(
    chain: Chain,
    network: Network,
    vault_pubkey: &[u8; 33],
    chain_code: &[u8; 32],
    index: u64,
) -> Result<String, Error> {
    let child = hd::child_pubkey(vault_pubkey, chain_code, &hd::deposit_path(chain, index))?;
    address_of(chain, network, &child.public_key)
}

pub fn address_of(chain: Chain, network: Network, pubkey33: &[u8; 33]) -> Result<String, Error> {
    match chain {
        Chain::Bitcoin => btc_address(pubkey33, network),
        Chain::Ethereum => eth_address(pubkey33),
        Chain::Tron => tron_address(pubkey33),
    }
}

pub fn validate_address(chain: Chain, network: Network, addr: &str) -> bool {
    match chain {
        Chain::Bitcoin => BtcAddress::from_str(addr)
            .ok()
            .and_then(|a| a.require_network(network.bitcoin()).ok())
            .is_some(),
        Chain::Ethereum => eth_decode(addr).is_ok(),
        Chain::Tron => tron_decode(addr).is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};

    fn pubkey_of(secret: u64) -> [u8; 33] {
        let mut sk = [0u8; 32];
        sk[24..].copy_from_slice(&secret.to_be_bytes());
        let secp = Secp256k1::new();
        bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&sk).unwrap())
            .serialize()
    }

    #[test]
    fn eth_address_of_private_key_one() {
        assert_eq!(
            eth_address(&pubkey_of(1)).unwrap(),
            "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf"
        );
        // EIP-55 vector.
        let a = eth_decode("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed").unwrap();
        assert_eq!(
            eth_checksum(&a),
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
        );
        assert!(eth_decode("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD").is_err());
        assert!(eth_decode("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").is_ok());
    }

    #[test]
    fn tron_usdt_contract_address_round_trips() {
        let account = hex::decode("a614f803b6fd780986a42c78ec9c7f77e6ded13c").unwrap();
        let account: [u8; 20] = account.try_into().unwrap();
        let addr = tron_address_from_account(&account);
        assert_eq!(addr, "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t");
        assert_eq!(&tron_decode(&addr).unwrap()[1..], &account);
        assert!(validate_address(Chain::Tron, Network::Mainnet, &addr));
        assert!(!validate_address(
            Chain::Tron,
            Network::Mainnet,
            "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6u"
        ));
    }

    #[test]
    fn btc_p2wpkh_per_network() {
        let pk = pubkey_of(1);
        let main = btc_address(&pk, Network::Mainnet).unwrap();
        let test = btc_address(&pk, Network::Testnet).unwrap();
        let reg = btc_address(&pk, Network::Regtest).unwrap();
        assert!(main.starts_with("bc1q"));
        assert!(test.starts_with("tb1q"));
        assert!(reg.starts_with("bcrt1q"));
        assert!(validate_address(Chain::Bitcoin, Network::Mainnet, &main));
        assert!(!validate_address(Chain::Bitcoin, Network::Mainnet, &test));
        assert!(validate_address(Chain::Bitcoin, Network::Signet, &test));
    }

    #[test]
    fn deposit_addresses_differ_per_index_and_chain() {
        let pk = pubkey_of(7);
        let cc = [9u8; 32];
        let a0 = deposit_address(Chain::Bitcoin, Network::Regtest, &pk, &cc, 0).unwrap();
        let a1 = deposit_address(Chain::Bitcoin, Network::Regtest, &pk, &cc, 1).unwrap();
        let e0 = deposit_address(Chain::Ethereum, Network::Mainnet, &pk, &cc, 0).unwrap();
        let t0 = deposit_address(Chain::Tron, Network::Mainnet, &pk, &cc, 0).unwrap();
        assert_ne!(a0, a1);
        assert!(e0.starts_with("0x"));
        assert!(t0.starts_with('T'));
    }
}
