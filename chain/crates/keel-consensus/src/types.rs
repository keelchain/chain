//! Consensus-level type aliases. One place to change when the scheme or
//! digest changes.

use commonware_consensus::{
    simplex::{
        scheme,
        types::{Activity as CActivity, Context as CContext, Finalization as CFinalization},
    },
    types::Epoch,
};
use commonware_cryptography::{ed25519, sha256};
use commonware_utils::NZU64;
use std::num::NonZeroU64;

/// Namespace prefix for every consensus signature (replay protection across
/// chains that share keys).
pub const NAMESPACE: &[u8] = b"_KEEL_CHAIN";

/// Single epoch until Phase 2 introduces validator-set changes.
pub const EPOCH: Epoch = Epoch::zero();
pub const EPOCH_LENGTH: NonZeroU64 = NZU64!(u64::MAX);

pub type PublicKey = ed25519::PublicKey;
pub type PrivateKey = ed25519::PrivateKey;
pub type Digest = sha256::Digest;
pub type Hasher = sha256::Sha256;

/// Plain ed25519 certificates: linear-size, batch-verifiable, no DKG.
pub type Scheme = scheme::ed25519::Scheme;
pub type Context = CContext<Digest, PublicKey>;
pub type Activity = CActivity<Scheme, Digest>;
pub type Finalization = CFinalization<Scheme, Digest>;
