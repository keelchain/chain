//! Chain addresses: 32 bytes, the ed25519 public key of a native account.
//! Reserved addresses (all-zero and friends) name protocol-owned accounts so
//! the ledger's "customer_id = system" convention survives as data.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct Address(pub [u8; 32]);

/// JSON: a 64-char hex string (what every RPC response and the SDK use).
/// Deserialization also accepts a 32-element byte array for older clients.
impl Serialize for Address {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Hex(String),
            Bytes(Vec<u8>),
        }
        match Repr::deserialize(d)? {
            Repr::Hex(h) => Address::from_hex(h.trim().trim_start_matches("0x"))
                .ok_or_else(|| serde::de::Error::custom("address must be 64 hex chars")),
            Repr::Bytes(b) => b
                .try_into()
                .map(Address)
                .map_err(|_| serde::de::Error::custom("address must be 32 bytes")),
        }
    }
}

impl Address {
    /// Protocol-owned accounts: fees, treasury, burn, vaults. The old
    /// ledger's `system` customer.
    pub const SYSTEM: Address = Address([0u8; 32]);

    pub fn from_bytes(b: [u8; 32]) -> Self {
        Address(b)
    }

    /// A deterministic test/genesis address from a small tag.
    pub fn tagged(tag: u64) -> Self {
        let mut b = [0u8; 32];
        b[24..].copy_from_slice(&tag.to_be_bytes());
        b[0] = 0xff;
        Address(b)
    }

    pub fn is_system(&self) -> bool {
        *self == Address::SYSTEM
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let v = hex::decode(s).ok()?;
        let b: [u8; 32] = v.try_into().ok()?;
        Some(Address(b))
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({}…)", &self.to_hex()[..8])
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip_and_tags_differ() {
        let a = Address::tagged(7);
        assert_eq!(Address::from_hex(&a.to_hex()), Some(a));
        assert_ne!(Address::tagged(7), Address::tagged(8));
        assert!(Address::SYSTEM.is_system());
        assert!(!a.is_system());
    }
}
