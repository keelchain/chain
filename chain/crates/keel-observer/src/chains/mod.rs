//! Node clients per chain, each behind a trait with an HTTP implementation
//! and a fixture-driven mock, plus the pure functions that turn node data
//! into `DepositObservation`s and signed transactions.

pub mod btc;
pub mod eth;
pub mod tron;

use crate::rpc::VaultView;
use keel_actions::Chain;
use keel_chains::{hd, Network};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

/// Deposit addresses of one vault: index → address and back. Index 0 is
/// never assigned to a user by the chain and serves as the vault's hot /
/// change address.
#[derive(Clone, Debug)]
pub struct AddressBook {
    pub chain: Chain,
    pub network: Network,
    pub vault: VaultView,
    by_index: BTreeMap<u64, String>,
    by_addr: HashMap<String, u64>,
}

impl AddressBook {
    /// Derive every index below `vault.next_deposit_index` (and index 0).
    pub fn build(vault: VaultView, network: Network) -> anyhow::Result<Self> {
        let mut book = Self {
            chain: vault.chain,
            network,
            vault,
            by_index: BTreeMap::new(),
            by_addr: HashMap::new(),
        };
        let upto = book.vault.next_deposit_index;
        for index in 0..upto {
            book.insert(index)?;
        }
        Ok(book)
    }

    fn insert(&mut self, index: u64) -> anyhow::Result<()> {
        let addr = keel_chains::deposit_address(
            self.chain,
            self.network,
            &self.vault.public_key,
            &self.vault.chain_code,
            index,
        )?;
        self.by_addr.insert(normalize(self.chain, &addr), index);
        self.by_index.insert(index, addr);
        Ok(())
    }

    pub fn address(&self, index: u64) -> Option<&str> {
        self.by_index.get(&index).map(String::as_str)
    }

    pub fn index_of(&self, addr: &str) -> Option<u64> {
        self.by_addr.get(&normalize(self.chain, addr)).copied()
    }

    pub fn indexes(&self) -> impl Iterator<Item = u64> + '_ {
        self.by_index.keys().copied()
    }

    pub fn addresses(&self) -> impl Iterator<Item = (u64, &str)> + '_ {
        self.by_index.iter().map(|(i, a)| (*i, a.as_str()))
    }

    pub fn len(&self) -> usize {
        self.by_index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_index.is_empty()
    }

    pub fn path(&self, index: u64) -> Vec<u32> {
        hd::deposit_path(self.chain, index)
    }

    pub fn pubkey(&self, index: u64) -> anyhow::Result<[u8; 33]> {
        Ok(hd::child_pubkey(
            &self.vault.public_key,
            &self.vault.chain_code,
            &self.path(index),
        )?
        .public_key)
    }

    /// Owner of a deposit index, if the chain assigned it.
    pub fn owner(&self, index: u64) -> Option<keel_types::Address> {
        self.vault.owners.get(&index).copied()
    }
}

fn normalize(chain: Chain, addr: &str) -> String {
    match chain {
        Chain::Ethereum | Chain::Bitcoin => addr.to_ascii_lowercase(),
        _ => addr.to_string(),
    }
}

/// Parse a decimal string (`"0.00012345"`, `"1e-05"`, `"12"`) into
/// integer smallest units with `decimals` places, flooring. Floats are
/// never used so amounts round-trip exactly.
pub fn decimal_to_units(s: &str, decimals: u32) -> Option<u128> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let (mantissa, exp) = match s.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().ok()?),
        None => (s, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    // digits with the decimal point removed; scale = number of fractional digits - exp
    let digits = format!("{int_part}{frac_part}");
    let scale = frac_part.len() as i32 - exp;
    let target = decimals as i32;
    let value: u128 = if digits.is_empty() {
        0
    } else {
        digits.trim_start_matches('0').parse().unwrap_or(0)
    };
    if value == 0 {
        return Some(0);
    }
    if scale <= target {
        let shift = (target - scale) as u32;
        value.checked_mul(10u128.checked_pow(shift)?)
    } else {
        let shift = (scale - target) as u32;
        Some(value / 10u128.checked_pow(shift)?)
    }
}

/// A `serde_json` number or string as decimal smallest units.
pub fn json_amount(v: &Value, decimals: u32) -> Option<u128> {
    match v {
        Value::Number(n) => decimal_to_units(&n.to_string(), decimals),
        Value::String(s) => decimal_to_units(s, decimals),
        _ => None,
    }
}

/// Hex quantity (`0x1a`) or decimal string/number to u128.
pub fn json_quantity(v: &Value) -> Option<u128> {
    match v {
        Value::Number(n) => n.as_u128(),
        Value::String(s) => {
            if let Some(h) = s.strip_prefix("0x") {
                u128::from_str_radix(h, 16).ok()
            } else {
                s.parse().ok()
            }
        }
        _ => None,
    }
}

pub fn json_hex(v: &Value) -> Option<Vec<u8>> {
    let s = v.as_str()?;
    hex::decode(s.strip_prefix("0x").unwrap_or(s)).ok()
}

pub fn json_hex32(v: &Value) -> Option<[u8; 32]> {
    json_hex(v)?.try_into().ok()
}

pub fn json_hex20(v: &Value) -> Option<[u8; 20]> {
    json_hex(v)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decimal_parsing_is_exact() {
        assert_eq!(decimal_to_units("0.00012345", 8), Some(12_345));
        assert_eq!(decimal_to_units("1e-05", 8), Some(1_000));
        assert_eq!(decimal_to_units("12", 8), Some(1_200_000_000));
        assert_eq!(decimal_to_units("0.123456789", 8), Some(12_345_678));
        assert_eq!(decimal_to_units("1.5e2", 0), Some(150));
        assert_eq!(decimal_to_units("0", 8), Some(0));
        assert_eq!(decimal_to_units("-1", 8), None);
        assert_eq!(decimal_to_units("abc", 8), None);
        assert_eq!(json_amount(&json!(0.0001), 8), Some(10_000));
        assert_eq!(json_amount(&json!(0.00001), 8), Some(1_000));
        assert_eq!(json_quantity(&json!("0x1a")), Some(26));
        assert_eq!(json_quantity(&json!("26")), Some(26));
        assert_eq!(json_quantity(&json!(26)), Some(26));
    }

    #[test]
    fn address_book_round_trips() {
        let vault = VaultView {
            chain: Chain::Ethereum,
            epoch: 1,
            public_key: {
                let secp = secp256k1::Secp256k1::new();
                secp256k1::PublicKey::from_secret_key(
                    &secp,
                    &secp256k1::SecretKey::from_slice(&[3u8; 32]).unwrap(),
                )
                .serialize()
            },
            chain_code: [4u8; 32],
            signers: vec![],
            threshold: 1,
            next_deposit_index: 3,
            owners: BTreeMap::from([(1, keel_types::Address::tagged(1))]),
        };
        let book = AddressBook::build(vault, Network::Regtest).unwrap();
        assert_eq!(book.len(), 3);
        let a1 = book.address(1).unwrap().to_string();
        assert_eq!(book.index_of(&a1.to_ascii_lowercase()), Some(1));
        assert_eq!(
            book.index_of(&a1.to_ascii_uppercase().replace("0X", "0x")),
            Some(1)
        );
        assert_eq!(book.owner(1), Some(keel_types::Address::tagged(1)));
        assert_eq!(book.owner(2), None);
        assert_eq!(book.path(2), vec![60, 2]);
        assert_eq!(
            keel_chains::address::eth_address(&book.pubkey(1).unwrap()).unwrap(),
            a1
        );
    }
}
