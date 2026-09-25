//! Observer-signer daemon for the KEEL vaults (docs/plan.md §4).
//!
//! Loops: [`daemon`] wires them; [`deposits`] scans the external chains
//! and submits `ObserveDeposit`; [`outbound`] builds, signs (through the
//! TSS daemon), broadcasts and confirms withdrawals; [`fees`] reports
//! network fee rates. Node access is behind traits in [`chains`] and
//! [`rpc`] with fixture mocks for tests.
//!
//! This crate runs above the VM: tokio, `HashMap` and the wall clock are
//! fine here (dev-rules.md applies to `keel-vm` and below).
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod chains;
pub mod config;
pub mod daemon;
pub mod deposits;
pub mod events;
pub mod fees;
pub mod lightning;
pub mod outbound;
pub mod rpc;
pub mod state;
pub mod tss;
