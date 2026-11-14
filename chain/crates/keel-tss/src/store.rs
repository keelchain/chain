//! Encrypted key-share files.
//!
//! Layout (JSON): a header with the KDF salt and iteration count, the
//! AEAD nonce, and the ciphertext of the serialized share. The key is
//! PBKDF2-HMAC-SHA256(passphrase, salt, iterations); the AEAD is
//! ChaCha20-Poly1305 with the header bytes as associated data, so the
//! metadata cannot be swapped without detection.

use crate::{
    ecdsa::{chain_code, vault_public_key, KeyShare},
    eddsa::{self, EddsaShare},
};
use cggmp21::key_share::AnyKeyShare as _;
use chacha20poly1305::{
    aead::{Aead as _, KeyInit as _},
    ChaCha20Poly1305, Nonce,
};
use hmac::{Hmac, Mac as _};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::Path;

pub const PASSPHRASE_ENV: &str = "KEEL_TSS_PASSPHRASE";
pub const DEFAULT_ITERATIONS: u32 = 200_000;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed share file: {0}")]
    Malformed(String),
    #[error("wrong passphrase or corrupted share")]
    Decrypt,
    #[error("{0} is not set")]
    MissingEnv(&'static str),
    #[error("share does not validate: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub kdf: String,
    pub salt: String,
    pub iterations: u32,
    /// Party index at keygen.
    pub index: u16,
    pub n: u16,
    pub t: u16,
    /// Vault key, hex: compressed secp256k1 (33 bytes) for `ecdsa`,
    /// Ed25519 (32 bytes) for `eddsa`.
    pub public_key: String,
    pub chain_code: Option<String>,
    /// `ecdsa` (CGGMP21, the default of files written before Solana) or
    /// `eddsa` (FROST).
    #[serde(default)]
    pub scheme: Option<String>,
}

impl Header {
    pub fn scheme(&self) -> &str {
        self.scheme.as_deref().unwrap_or("ecdsa")
    }
}

#[derive(Serialize, Deserialize)]
struct ShareFile {
    header: Header,
    nonce: String,
    ciphertext: String,
}

/// PBKDF2-HMAC-SHA256, single 32-byte block.
pub fn pbkdf2_sha256(passphrase: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(passphrase)
        .expect("hmac accepts any key length");
    mac.update(salt);
    mac.update(&1u32.to_be_bytes());
    let mut u: [u8; 32] = mac.finalize().into_bytes().into();
    let mut out = u;
    for _ in 1..iterations.max(1) {
        let mut mac = <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(passphrase)
            .expect("hmac accepts any key length");
        mac.update(&u);
        u = mac.finalize().into_bytes().into();
        for (o, x) in out.iter_mut().zip(u.iter()) {
            *o ^= x;
        }
    }
    out
}

pub fn passphrase_from_env() -> Result<String, StoreError> {
    std::env::var(PASSPHRASE_ENV).map_err(|_| StoreError::MissingEnv(PASSPHRASE_ENV))
}

fn header_bytes(h: &Header) -> Vec<u8> {
    serde_json::to_vec(h).unwrap_or_default()
}

/// Encrypt a CGGMP21 `share` under `passphrase` into the JSON document.
pub fn encrypt_share(
    share: &KeyShare,
    passphrase: &str,
    iterations: u32,
) -> Result<String, StoreError> {
    let plaintext = serde_json::to_vec(share).map_err(|e| StoreError::Malformed(e.to_string()))?;
    encrypt_bytes(
        "ecdsa",
        share.i,
        share.n(),
        share.min_signers(),
        &vault_public_key(share),
        chain_code(share),
        &plaintext,
        passphrase,
        iterations,
    )
}

/// Encrypt a FROST `share` under `passphrase` into the JSON document.
pub fn encrypt_eddsa_share(
    share: &EddsaShare,
    passphrase: &str,
    iterations: u32,
) -> Result<String, StoreError> {
    let plaintext = serde_json::to_vec(share).map_err(|e| StoreError::Malformed(e.to_string()))?;
    encrypt_bytes(
        "eddsa",
        share.index,
        share.n,
        share.t,
        &eddsa::vault_public_key(share),
        Some(share.chain_code),
        &plaintext,
        passphrase,
        iterations,
    )
}

#[allow(clippy::too_many_arguments)]
fn encrypt_bytes(
    scheme: &str,
    index: u16,
    n: u16,
    t: u16,
    public_key: &[u8],
    chain_code: Option<[u8; 32]>,
    plaintext: &[u8],
    passphrase: &str,
    iterations: u32,
) -> Result<String, StoreError> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let header = Header {
        version: 1,
        kdf: "pbkdf2-hmac-sha256".into(),
        salt: hex::encode(salt),
        iterations,
        index,
        n,
        t,
        public_key: hex::encode(public_key),
        chain_code: chain_code.map(hex::encode),
        scheme: Some(scheme.into()),
    };
    let key = pbkdf2_sha256(passphrase.as_bytes(), &salt, iterations);
    let cipher = ChaCha20Poly1305::new((&key).into());
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad: &header_bytes(&header),
            },
        )
        .map_err(|_| StoreError::Decrypt)?;
    let doc = ShareFile {
        header,
        nonce: hex::encode(nonce),
        ciphertext: hex::encode(ct),
    };
    serde_json::to_string_pretty(&doc).map_err(|e| StoreError::Malformed(e.to_string()))
}

/// Decrypt a JSON document produced by [`encrypt_share`].
pub fn decrypt_share(doc: &str, passphrase: &str) -> Result<KeyShare, StoreError> {
    let (header, pt) = decrypt_bytes(doc, passphrase)?;
    if header.scheme() != "ecdsa" {
        return Err(StoreError::Invalid(format!(
            "share file holds a {} key, not an ecdsa one",
            header.scheme()
        )));
    }
    let share: KeyShare =
        serde_json::from_slice(&pt).map_err(|e| StoreError::Invalid(e.to_string()))?;
    if share.i != header.index {
        return Err(StoreError::Invalid(
            "header index does not match the share".into(),
        ));
    }
    Ok(share)
}

/// Decrypt a JSON document produced by [`encrypt_eddsa_share`].
pub fn decrypt_eddsa_share(doc: &str, passphrase: &str) -> Result<EddsaShare, StoreError> {
    let (header, pt) = decrypt_bytes(doc, passphrase)?;
    if header.scheme() != "eddsa" {
        return Err(StoreError::Invalid(format!(
            "share file holds a {} key, not an eddsa one",
            header.scheme()
        )));
    }
    let share: EddsaShare =
        serde_json::from_slice(&pt).map_err(|e| StoreError::Invalid(e.to_string()))?;
    if share.index != header.index {
        return Err(StoreError::Invalid(
            "header index does not match the share".into(),
        ));
    }
    Ok(share)
}

fn decrypt_bytes(doc: &str, passphrase: &str) -> Result<(Header, Vec<u8>), StoreError> {
    let f: ShareFile =
        serde_json::from_str(doc).map_err(|e| StoreError::Malformed(e.to_string()))?;
    if f.header.version != 1 || f.header.kdf != "pbkdf2-hmac-sha256" {
        return Err(StoreError::Malformed("unsupported version or kdf".into()));
    }
    let salt = hex::decode(&f.header.salt).map_err(|e| StoreError::Malformed(e.to_string()))?;
    let nonce = hex::decode(&f.nonce).map_err(|e| StoreError::Malformed(e.to_string()))?;
    let ct = hex::decode(&f.ciphertext).map_err(|e| StoreError::Malformed(e.to_string()))?;
    if nonce.len() != 12 {
        return Err(StoreError::Malformed("nonce must be 12 bytes".into()));
    }
    let key = pbkdf2_sha256(passphrase.as_bytes(), &salt, f.header.iterations);
    let cipher = ChaCha20Poly1305::new((&key).into());
    let pt = cipher
        .decrypt(
            Nonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &ct,
                aad: &header_bytes(&f.header),
            },
        )
        .map_err(|_| StoreError::Decrypt)?;
    Ok((f.header, pt))
}

/// Read only the (unauthenticated until decrypted) header of a share file.
pub fn read_header(path: &Path) -> Result<Header, StoreError> {
    let doc = std::fs::read_to_string(path)?;
    let f: ShareFile =
        serde_json::from_str(&doc).map_err(|e| StoreError::Malformed(e.to_string()))?;
    Ok(f.header)
}

pub fn save_share(path: &Path, share: &KeyShare, passphrase: &str) -> Result<(), StoreError> {
    let doc = encrypt_share(share, passphrase, DEFAULT_ITERATIONS)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, doc)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_share(path: &Path, passphrase: &str) -> Result<KeyShare, StoreError> {
    let doc = std::fs::read_to_string(path)?;
    decrypt_share(&doc, passphrase)
}

pub fn save_eddsa_share(
    path: &Path,
    share: &EddsaShare,
    passphrase: &str,
) -> Result<(), StoreError> {
    let doc = encrypt_eddsa_share(share, passphrase, DEFAULT_ITERATIONS)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, doc)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn load_eddsa_share(path: &Path, passphrase: &str) -> Result<EddsaShare, StoreError> {
    let doc = std::fs::read_to_string(path)?;
    decrypt_eddsa_share(&doc, passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbkdf2_matches_rfc_style_vector() {
        // PBKDF2-HMAC-SHA256("password", "salt", 1) first 32 bytes.
        let k = pbkdf2_sha256(b"password", b"salt", 1);
        assert_eq!(
            hex::encode(k),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        let k2 = pbkdf2_sha256(b"password", b"salt", 2);
        assert_eq!(
            hex::encode(k2),
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"
        );
    }
}
