//! Shared vocabulary of the Keelchain. Everything here is plain data and
//! integer arithmetic: no I/O, no clocks, no randomness, no floats.
//!
//! Money rule (inherited from services/ledger and services/orderbook): every
//! amount is an INTEGER count of an asset's smallest units. A price is the
//! number of QUOTE smallest units per ONE WHOLE base unit. All rounding is
//! floor, so the chain never pays out a unit it did not receive.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod address;
pub mod amount;
pub mod asset;
pub mod ids;
pub mod market;

pub use address::Address;
pub use amount::{
    base_for_quote, fee_of, floor_to_lot, mul_div_floor, pow10, quote_amount, Amount,
};
pub use asset::Asset;
pub use ids::{BlockHeight, OrderId, Seq, TxSeq};
pub use market::{FeeSide, OrderType, PairConfig, SettleParty, Side};
