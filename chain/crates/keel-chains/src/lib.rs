//! Per-chain address derivation and unsigned transaction builders for the
//! KEEL vaults. Pure functions only: no I/O, no clocks. The observer daemon
//! feeds these with node data; the TSS signs the digests they return.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod address;
pub mod btc;
pub mod eth;
pub mod hd;
pub mod policy;
pub mod tron;

pub use address::{deposit_address, validate_address};
pub use hd::{chain_index, child_pubkey, DerivedKey};
use serde::{Deserialize, Serialize};

/// Which network of each chain the vault lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
}

impl Network {
    pub fn bitcoin(self) -> bitcoin::Network {
        match self {
            Network::Mainnet => bitcoin::Network::Bitcoin,
            Network::Testnet => bitcoin::Network::Testnet,
            Network::Signet => bitcoin::Network::Signet,
            Network::Regtest => bitcoin::Network::Regtest,
        }
    }

    /// EIP-155 chain id: mainnet 1, everything else Sepolia 11155111 (a
    /// local devnet overrides through [`eth::Eip1559Tx::chain_id`]).
    pub fn eth_chain_id(self) -> u64 {
        match self {
            Network::Mainnet => 1,
            _ => 11_155_111,
        }
    }
}

/// Estimated fee for one chain, in the chain's native unit of fee rate:
/// sat/vB (BTC), wei per gas (ETH), sun per tx (TRON).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeRate {
    pub chain: keel_actions::Chain,
    pub rate: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid key: {0}")]
    Key(String),
    #[error("invalid address: {0}")]
    Address(String),
    #[error("unsupported chain {0:?}")]
    Unsupported(keel_actions::Chain),
    #[error("insufficient funds: need {need}, have {have}")]
    InsufficientFunds { need: u64, have: u64 },
    #[error("invalid transaction: {0}")]
    Tx(String),
}
