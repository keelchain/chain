//! Order-book vocabulary shared by `keel-book` and the markets module of the
//! VM. Ported from services/orderbook/src/domain.rs; the per-order records
//! stay in `keel-book`, and persistence types (timestamps, lock states) belong
//! to the VM.

use crate::{Address, Amount, Asset};
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    Limit,
    Market,
}

/// Which asset a fill's taker fee was taken in.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub enum FeeSide {
    Base,
    Quote,
}

/// Whose ledger account a synthetic level settles against.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct SettleParty {
    pub owner: Address,
    pub account_type: String,
}

/// A pair's trading rules and the ledger parties behind house liquidity and
/// fees. On chain this is governance data; decimals come from here, never
/// from code, so a new pair is one proposal away.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
pub struct PairConfig {
    pub symbol: String,
    pub base_asset: Asset,
    pub quote_asset: Asset,
    pub base_decimals: u32,
    pub quote_decimals: u32,
    pub tick_size: Amount,
    pub lot_size: Amount,
    pub min_notional: Amount,
    pub max_notional: Amount,
    pub taker_fee_bps: u32,
    pub maker_fee_bps: u32,
    pub enabled: bool,
    pub house_maker_enabled: bool,
    pub house_party: SettleParty,
    pub fee_party: SettleParty,
}
