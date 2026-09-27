//! One file per module. Each exposes a `*State` held in [`crate::State`],
//! an `apply` for its actions, and optionally `end_block`.
//!
//! Atomicity rule: validate everything, then take a ledger savepoint,
//! post, and only after every post succeeds mutate the module's own
//! state. On error return before touching module state; `apply.rs` rolls
//! the ledger back to the savepoint.

pub mod attest;
pub mod budgets;
pub mod clients;
pub mod custody;
pub mod disputes;
pub mod fees;
pub mod gov;
pub mod lightning;
pub mod markets;
pub mod p2p;
pub mod sessions;
pub mod stable;
pub mod staking;
pub mod tokens;
pub mod treasury;
pub mod vaults;
