//! Hashing and signatures. Pure functions only; no I/O, no clocks.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use keel_types::Address;
use sha2::{Digest as _, Sha256};

pub type Hash32 = [u8; 32];

pub fn sha256(parts: &[&[u8]]) -> Hash32 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// A native-account keypair. The address IS the ed25519 public key.
#[derive(Clone)]
pub struct Keypair(SigningKey);

impl Keypair {
    pub fn from_seed(seed: u64) -> Self {
        let secret = sha256(&[b"keel-keypair-seed", &seed.to_be_bytes()]);
        Keypair(SigningKey::from_bytes(&secret))
    }

    pub fn from_secret(secret: [u8; 32]) -> Self {
        Keypair(SigningKey::from_bytes(&secret))
    }

    pub fn generate(rng: &mut impl rand_core::CryptoRng) -> Self {
        let mut secret = [0u8; 32];
        rng.fill_bytes(&mut secret);
        Keypair(SigningKey::from_bytes(&secret))
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn address(&self) -> Address {
        Address(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message).to_bytes()
    }
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keypair({})", self.address())
    }
}

/// Verify an ed25519 signature by the account at `address`.
pub fn verify(address: &Address, message: &[u8], signature: &[u8; 64]) -> bool {
    // The system address is a sentinel, never a key, and weak points must
    // never authorize anything.
    if address.is_system() {
        return false;
    }
    let Ok(key) = VerifyingKey::from_bytes(&address.0) else {
        return false;
    };
    if key.is_weak() {
        return false;
    }
    key.verify_strict(message, &Signature::from_bytes(signature))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_round_trip() {
        let k = Keypair::from_seed(1);
        let sig = k.sign(b"hello");
        assert!(verify(&k.address(), b"hello", &sig));
        assert!(!verify(&k.address(), b"hellp", &sig));
        assert!(!verify(&Keypair::from_seed(2).address(), b"hello", &sig));
        assert_eq!(Keypair::from_seed(1).address(), k.address());
    }

    #[test]
    fn system_address_can_never_sign() {
        let sig = [0u8; 64];
        assert!(!verify(&Address::SYSTEM, b"x", &sig));
    }
}
