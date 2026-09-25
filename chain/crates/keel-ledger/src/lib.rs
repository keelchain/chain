//! The chain's double-entry ledger. A straight port of services/ledger with
//! Postgres replaced by deterministic in-memory state:
//!
//! - every transaction balances: sum(debits) == sum(credits);
//! - every transaction is single-asset;
//! - restricted accounts never go negative (`NotEnoughFunds`);
//! - every transaction is idempotent on a caller-supplied `external_id`, and
//!   the same id with a different payload is an `IdempotencyConflict`;
//! - `group_id` correlates the phases of one business operation;
//! - `reversal_of` posts the exact inverse of an earlier transaction.
//!
//! Money never moves except through [`Ledger::post`]. Escrow is a transfer
//! into a restricted account, never a lock.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod catalog;
pub mod error;
pub mod ledger;

pub use catalog::{account_type_info, AccountType, NormalSide, TxType, ACCOUNT_TYPES};
pub use error::LedgerError;
pub use ledger::{
    AccountKey, AccountState, AuditReport, Ledger, Posted, Record, RecordType, Savepoint,
};
