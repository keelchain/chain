//! Actions are the only way state changes. An [`Envelope`] names the signer,
//! a nonce, the chain, and one [`Action`]; a [`SignedAction`] adds the
//! ed25519 signature over `sha256("keel-action-v1" || borsh(envelope))`.
//!
//! There is no gas. Spam is bounded by per-address [`budget`]s that grow
//! with filled volume (docs/plan.md §2).
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod action;
pub mod budget;
pub mod envelope;
pub mod hex32;
pub mod payload;

pub use action::*;
pub use budget::{Budget, BudgetParams};
pub use envelope::{Envelope, SignedAction, CHAIN_ID_DEVNET};
pub use payload::{decode_payload, encode_payload, MAX_ACTIONS_PER_BLOCK};
