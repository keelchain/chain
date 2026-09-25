//! Deterministic identifiers. Nothing on the chain is a random UUID: every
//! id is a counter the state machine increments while applying a block, so
//! all validators derive the same ids for the same history.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! counter_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
            Default,
            borsh::BorshSerialize,
            borsh::BorshDeserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub fn next(self) -> Self {
                $name(self.0 + 1)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

counter_id!(OrderId, "Order id, assigned when an order is accepted.");
counter_id!(TxSeq, "Ledger transaction sequence, global and gap-free.");
counter_id!(Seq, "Time-priority sequence inside the order book.");
counter_id!(BlockHeight, "Height of a finalized block.");
