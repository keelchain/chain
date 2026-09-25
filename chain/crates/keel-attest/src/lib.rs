//! Observer-quorum attestations. An [`Attestation`] is the canonical
//! description of one external-chain event; observers submit it inside a
//! signed action, so the envelope already proves who said it. This crate
//! only defines the digest and counts distinct votes.
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::Chain;
use keel_crypto::{sha256, Hash32};
use keel_types::{Address, Amount, Asset};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const DOMAIN: &[u8] = b"attest-v1";

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Attestation {
    pub chain: Chain,
    pub tx_hash: Hash32,
    pub index: u32,
    pub deposit_index: u64,
    pub amount: Amount,
    pub asset: Asset,
    pub external_height: u64,
}

impl Attestation {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        borsh::to_vec(self).unwrap_or_default()
    }

    pub fn digest(&self) -> Hash32 {
        sha256(&[DOMAIN, &self.canonical_bytes()])
    }
}

/// Distinct observer votes on one digest.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Quorum {
    pub voters: BTreeSet<Address>,
}

impl Quorum {
    /// Records a vote; returns false when the observer already voted.
    pub fn vote(&mut self, observer: Address) -> bool {
        self.voters.insert(observer)
    }

    pub fn count(&self) -> u32 {
        self.voters.len() as u32
    }

    /// Votes only count from the current observer set.
    pub fn count_within(&self, observers: &[Address]) -> u32 {
        self.voters.iter().filter(|v| observers.contains(v)).count() as u32
    }

    pub fn reached(&self, observers: &[Address], threshold: u32) -> bool {
        self.count_within(observers) >= threshold.max(1)
    }
}

/// Threshold in votes for `bps` of `n` observers, rounded up, at least 1.
pub fn threshold_for(n: u32, bps: u32) -> u32 {
    let needed = (n as u64 * bps as u64).div_ceil(10_000) as u32;
    needed.clamp(1, n.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(amount: Amount) -> Attestation {
        Attestation {
            chain: Chain::Bitcoin,
            tx_hash: [7u8; 32],
            index: 1,
            deposit_index: 3,
            amount,
            asset: Asset::vault("BTC", "BTC"),
            external_height: 100,
        }
    }

    #[test]
    fn digest_binds_every_field() {
        assert_eq!(att(5).digest(), att(5).digest());
        assert_ne!(att(5).digest(), att(6).digest());
        let mut other = att(5);
        other.index = 2;
        assert_ne!(att(5).digest(), other.digest());
    }

    #[test]
    fn quorum_counts_distinct_current_observers() {
        let a = Address::tagged(1);
        let b = Address::tagged(2);
        let stranger = Address::tagged(9);
        let set = vec![a, b, Address::tagged(3)];
        let mut q = Quorum::default();
        assert!(q.vote(a));
        assert!(!q.vote(a));
        assert!(q.vote(stranger));
        assert_eq!(q.count(), 2);
        assert_eq!(q.count_within(&set), 1);
        assert!(!q.reached(&set, 2));
        q.vote(b);
        assert!(q.reached(&set, 2));
        assert_eq!(threshold_for(3, 6_667), 3);
        assert_eq!(threshold_for(4, 6_667), 3);
        assert_eq!(threshold_for(9, 6_667), 7);
        assert_eq!(threshold_for(0, 6_667), 1);
    }
}
