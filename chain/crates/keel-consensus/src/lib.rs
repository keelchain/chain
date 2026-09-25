//! Commonware simplex adapter for the Keelchain.
//!
//! Everything consensus-specific lives here so the rest of the workspace
//! never names a Commonware type: `keel-vm` implements [`StateMachine`], and
//! `keel-node` calls [`engine::Engine`]. Swapping the BFT engine (Malachite is
//! the fallback in docs/plan.md §1) means rewriting this crate only.
//!
//! Layout:
//! - [`types`]: the signing scheme (ed25519), digest, namespace, epoch.
//! - [`block`]: the block type consensus orders (context, parent, height,
//!   timestamp, opaque payload).
//! - [`application`]: proposes and verifies blocks, and applies finalized
//!   blocks in order to the [`StateMachine`].
//! - [`engine`]: constructs and starts broadcast, marshal and consensus.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod application;
pub mod block;
pub mod engine;
pub mod epochs;
pub mod types;

pub use application::{Application, StateMachine};
pub use block::Block;

#[cfg(test)]
mod tests;
