//! Snapshot framing: a snapshot round-trips through its header, a snapshot
//! of any other schema is refused, and the checked-in fixture of the
//! current schema (a devnet genesis) loads. There is no migration path
//! before mainnet: a layout change restarts the networks from genesis.
#![allow(clippy::unwrap_used)]

use keel_actions::CHAIN_ID_DEVNET;
use keel_crypto::Keypair;
use keel_vm::{genesis::GenesisValidator, migrate, Genesis, State};
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
    let (schema, _) = migrate::split(&bytes);
    assert_eq!(schema, migrate::SCHEMA);
    let back = State::restore(&bytes).expect("round trip");
    assert_eq!(back.compute_hash(), state.compute_hash());
}

#[test]
fn other_schemas_are_refused() {
    let state = genesis_state();
    let payload = borsh::to_vec(&state).unwrap();
    let mut newer = Vec::new();
    newer.extend_from_slice(migrate::MAGIC);
    newer.extend_from_slice(&(migrate::SCHEMA + 1).to_le_bytes());
    newer.extend_from_slice(&payload);
    assert!(State::restore(&newer).is_none());
    // Headerless bytes are schema 0: refused as well.
    assert!(State::restore(&payload).is_none());
}

/// A fixture of the current schema, written the first time this runs after
/// a schema bump (or with `KEEL_WRITE_FIXTURES=1`). It is the only fixture:
/// older ones were deleted with the migration path.
#[test]
fn the_current_schema_fixture_loads() {
    let path = fixtures_dir().join(format!("snapshot-schema{}-devnet.bin", migrate::SCHEMA));
    if std::env::var_os("KEEL_WRITE_FIXTURES").is_some() || !path.exists() {
        std::fs::create_dir_all(fixtures_dir()).unwrap();
        std::fs::write(&path, genesis_state().snapshot()).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    let state = State::restore(&bytes).expect("fixture of the current schema loads");
    assert!(state.ledger.audit().mismatches.is_empty());
    for entry in std::fs::read_dir(fixtures_dir()).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(
            !name.starts_with("snapshot-schema")
                || name == path.file_name().unwrap().to_string_lossy(),
            "{name}: fixtures of other schemas must not exist"
        );
    }
}
