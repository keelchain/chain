//! The KEEL state machine. `apply_block` is the only entry point that
//! mutates [`State`]; everything in here is deterministic (no clocks, no
//! randomness, no floats, ordered maps only).
//!
//! Module map (docs/plan.md §3): tokens, markets, budgets, p2p
//! (offers + trades), disputes, vaults, stable, staking, gov, attest.
//! Each module owns a sub-state in [`State`] and an `apply` entry in
//! `modules/`; `apply.rs` does admission (signature, nonce, budget, pause)
//! and dispatch.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod apply;
pub mod context;
pub mod genesis;
pub mod modules;
pub mod params;
pub mod receipt;
pub mod state;

pub use apply::{apply_block, check_admission};
pub use context::BlockContext;
pub use genesis::{Genesis, GenesisAccount};
pub use params::Params;
pub use receipt::{Event, Receipt, VmError};
pub use state::{AccountMeta, State};
