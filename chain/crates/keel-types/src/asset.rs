//! Asset identifiers. Native assets are bare symbols (`KEEL`, the USD stable);
//! vault assets carry their home chain (`BTC.BTC`, `ETH.USDT`, `TRON.USDT`),
//! the THORChain "secured asset" convention, so the same symbol on two chains
//! is never one balance.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    borsh::BorshSerialize,
    borsh::BorshDeserialize,
)]
#[serde(transparent)]
pub struct Asset(String);

impl Asset {
    pub fn new(symbol: impl Into<String>) -> Self {
        Asset(symbol.into())
    }

    /// `chain.symbol` for an asset custodied in a vault on another chain.
    pub fn vault(chain: &str, symbol: &str) -> Self {
        Asset(format!("{chain}.{symbol}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The home chain of a vault asset, `None` for a native one.
    pub fn chain(&self) -> Option<&str> {
        self.0.split_once('.').map(|(c, _)| c)
    }

    pub fn symbol(&self) -> &str {
        self.0.split_once('.').map(|(_, s)| s).unwrap_or(&self.0)
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Asset {
    fn from(s: &str) -> Self {
        Asset(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_assets_carry_their_chain() {
        let a = Asset::vault("ETH", "USDT");
        assert_eq!(a.as_str(), "ETH.USDT");
        assert_eq!(a.chain(), Some("ETH"));
        assert_eq!(a.symbol(), "USDT");
        assert_ne!(a, Asset::vault("TRON", "USDT"));
        let n = Asset::new("KEEL");
        assert_eq!(n.chain(), None);
        assert_eq!(n.symbol(), "KEEL");
    }
}
