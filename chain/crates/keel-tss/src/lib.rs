//! Threshold signer for the KEEL vaults.
//!
//! - [`ecdsa`]: CGGMP21 t-of-n ECDSA on secp256k1 with non-hardened
//!   SLIP-10/BIP32 child derivation (the deposit addresses derived by
//!   `keel-chains` from the registered vault key).
//! - [`eddsa`]: FROST t-of-n Ed25519 for Solana, with the additive child
//!   derivation of `keel_chains::sol`.
//! - [`transport`]: how parties exchange round messages (in-memory for
//!   tests, HMAC-authenticated TCP JSON lines between signer hosts).
//! - [`store`]: passphrase-encrypted share files.
//!
//! This crate runs on the signer host, above the VM: threads, wall clock
//! and `HashMap` are fine here (dev-rules.md applies to `keel-vm` and
//! below).
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod ecdsa;
pub mod eddsa;
pub mod protocol;
pub mod store;
pub mod transport;

pub use ecdsa::{
    chain_code, child_public_key, keygen, sign, sign_with_mailbox, vault_public_key,
    EcdsaSignature, KeyShare, KeygenParams,
};
pub use eddsa::EddsaShare;
pub use store::{load_eddsa_share, load_share, save_eddsa_share, save_share};
pub use transport::{InMemoryNetwork, TcpTransport, Transport, WireMessage};
