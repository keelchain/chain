//! Snapshot schema: headerless (schema 0) fixtures keep loading, the header
//! round-trips, and an unknown schema is refused.
#![allow(clippy::unwrap_used)]

use keel_actions::CHAIN_ID_DEVNET;
use keel_crypto::Keypair;
use keel_vm::{
    genesis::{Genesis, GenesisValidator},
    migrate::{self, MAGIC, SCHEMA},
    State,
};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn genesis_state() -> State {
    let v = Keypair::from_seed(100);
    Genesis::devnet(
        CHAIN_ID_DEVNET,
        &[Keypair::from_seed(1).address()],
        vec![GenesisValidator {
            address: v.address(),
            consensus_key: v.address().0,
            bond: 0,
        }],
    )
    .build()
}

#[test]
fn snapshot_has_header_and_round_trips() {
    let state = genesis_state();
    let bytes = state.snapshot();
    assert_eq!(&bytes[..4], MAGIC);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), SCHEMA);
    let back = State::restore(&bytes).expect("restore");
    assert_eq!(back.compute_hash(), state.compute_hash());
    assert_eq!(back.height, state.height);
}

#[test]
fn headerless_bytes_are_schema_zero_and_the_current_layout_is_not() {
    // Headerless bytes are read as schema 0. The current layout differs
    // from schema 0, so plain borsh of today's `State` must be refused
    // rather than misread; the checked-in schema-0 fixture is what loads.
    let state = genesis_state();
    let headerless = borsh::to_vec(&state).unwrap();
    assert!(State::restore(&headerless).is_none());
    let (schema, _) = migrate::split(&headerless);
    assert_eq!(schema, 0);
}

#[test]
fn newer_schema_is_refused() {
    let state = genesis_state();
    let payload = borsh::to_vec(&state).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(SCHEMA + 1).to_le_bytes());
    bytes.extend_from_slice(&payload);
    assert!(State::restore(&bytes).is_none());
}

/// The checked-in fixture is a schema-0 (headerless) devnet genesis snapshot
/// written by this test the first time it ran. It must keep loading after
/// every layout change through `migrate::upgrade`; when it stops loading,
/// the layout changed without a migration step. Regenerate only with
/// `KEEL_WRITE_FIXTURES=1` and a deliberate schema bump.
#[test]
fn schema_zero_fixture_loads() {
    let path = fixtures_dir().join("snapshot-schema0-devnet.bin");
    if !path.exists() {
        std::fs::create_dir_all(fixtures_dir()).unwrap();
        std::fs::write(&path, legacy_body(0)).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    let (schema, _) = migrate::split(&bytes);
    assert_eq!(schema, 0, "fixture must stay headerless");
    let state = State::restore(&bytes).expect("schema-0 fixture loads");
    assert_eq!(state.chain_id, CHAIN_ID_DEVNET);
    assert!(state.ledger.audit().mismatches.is_empty());
    // Re-framing the upgraded state yields the current schema.
    let (schema, _) = migrate::split(&state.snapshot());
    assert_eq!(schema, SCHEMA);
}

/// The genesis state serialized in an older top-level layout. Every schema
/// so far only appended fields to `State`, and borsh writes fields in
/// order, so an older body is the current body without its tail.
fn legacy_body(schema: u32) -> Vec<u8> {
    let state = genesis_state();
    let full = borsh::to_vec(&state).unwrap();
    let custody = borsh::to_vec(&state.custody).unwrap();
    let clients = borsh::to_vec(&state.clients).unwrap();
    let cut = match schema {
        0 | 1 => custody.len() + clients.len(),
        2 => custody.len(),
        _ => 0,
    };
    let body = full[..full.len() - cut].to_vec();
    if schema == 0 {
        return body;
    }
    let mut out = b"KEEL".to_vec();
    out.extend_from_slice(&schema.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Recovery tool: rewrite every older-schema fixture from the current
/// genesis (`cargo test -p keel-vm --test migrate write_legacy_fixtures --
/// --ignored`). Only needed when a fixture was lost or corrupted.
#[test]
#[ignore]
fn write_legacy_fixtures() {
    std::fs::create_dir_all(fixtures_dir()).unwrap();
    for schema in 0..SCHEMA {
        let path = fixtures_dir().join(format!("snapshot-schema{schema}-devnet.bin"));
        std::fs::write(&path, legacy_body(schema)).unwrap();
    }
}

/// A fixture of the CURRENT schema, written the first time this runs after a
/// schema bump (or with `KEEL_WRITE_FIXTURES=1`), so the next bump has an
/// old-schema snapshot to migrate from. Every fixture in the directory must
/// keep loading.
#[test]
fn every_schema_fixture_loads() {
    let path = fixtures_dir().join(format!("snapshot-schema{SCHEMA}-devnet.bin"));
    if std::env::var_os("KEEL_WRITE_FIXTURES").is_some() || !path.exists() {
        std::fs::create_dir_all(fixtures_dir()).unwrap();
        std::fs::write(&path, genesis_state().snapshot()).unwrap();
    }
    let mut seen = 0;
    for entry in std::fs::read_dir(fixtures_dir()).unwrap() {
        let p = entry.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("snapshot-schema") {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        let (schema, _) = migrate::split(&bytes);
        let state = State::restore(&bytes).unwrap_or_else(|| {
            panic!("{name} (schema {schema}) no longer loads: add a migration step")
        });
        assert!(state.ledger.audit().mismatches.is_empty(), "{name}");
        seen += 1;
    }
    assert!(
        seen >= 2,
        "expected the schema-0 and current fixtures, found {seen}"
    );
}
