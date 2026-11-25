//! Snapshot framing. A snapshot on disk is `b"KEEL" | u32 schema (little
//! endian) | borsh(State)`. There is no migration path: the chain is
//! pre-mainnet and every layout change restarts the networks from genesis
//! (2026-11-16), so a node loads a snapshot of exactly its own schema and
//! refuses any other. Snapshot migrations begin at mainnet.
//!
//! When the layout of `State` (or anything it contains) changes, bump
//! `SCHEMA` so no node misreads a snapshot written by another binary.

use crate::state::State;
use borsh::BorshDeserialize;

/// Current snapshot schema.
pub const SCHEMA: u32 = 20;

/// Magic prefix of a versioned snapshot.
pub const MAGIC: &[u8; 4] = b"KEEL";

/// Splits a snapshot into `(schema, payload)`. Headerless bytes are schema 0.
pub fn split(bytes: &[u8]) -> (u32, &[u8]) {
    if bytes.len() >= 8 && &bytes[..4] == MAGIC {
        let schema = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        (schema, &bytes[8..])
    } else {
        (0, bytes)
    }
}

/// Prefixes a payload with the current header.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&SCHEMA.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decodes a payload of the current schema. `None` when the bytes do not
/// decode or the schema is not this binary's.
pub fn upgrade(schema: u32, payload: &[u8]) -> Option<State> {
    if schema != SCHEMA {
        return None;
    }
    State::try_from_slice(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_recognises_header_and_headerless() {
        let (s, p) = split(&[1, 2, 3]);
        assert_eq!((s, p), (0, &[1u8, 2, 3][..]));
        let framed = frame(&[9, 9]);
        let (s, p) = split(&framed);
        assert_eq!((s, p), (SCHEMA, &[9u8, 9][..]));
    }

    #[test]
    fn only_the_current_schema_loads() {
        let state = State::default();
        let payload = borsh::to_vec(&state).unwrap();
        assert!(upgrade(SCHEMA, &payload).is_some());
        assert!(upgrade(SCHEMA + 1, &payload).is_none());
        assert!(upgrade(SCHEMA - 1, &payload).is_none());
        assert!(upgrade(0, &payload).is_none());
    }
}
