# Working rules for the Keelchain workspace

Read `plan.md` first. These rules keep parallel work from colliding.

## Ownership
- One crate or one `keel-vm/src/modules/<name>.rs` per workstream. Do not edit
  another workstream's files.
- Shared files (`keel-vm/src/{state,apply,receipt,params,genesis}.rs`,
  `keel-actions/src/*`, root `Cargo.toml`): use the **Edit** tool with a small,
  unique anchor; never rewrite them wholesale. Until mainnet there is no
  wire or snapshot compatibility to keep: a variant, field or action that
  is no longer wanted is deleted, not stubbed, and the networks restart
  from genesis (decided 2026-11-16). From mainnet on, `Event` variants,
  `Action` variants and `Params` fields are appended at the END of their
  lists and never renamed or removed.
- New external crates: add to your own crate's `[dependencies]` with an
  explicit version. Do not touch `[workspace.dependencies]`.

## Determinism (everything at `keel-vm` and below)
- No floats, no `HashMap`/`HashSet`, no wall clock, no randomness. Block time
  is `BlockContext::timestamp` (ms) / `seconds()`.
- Amounts are `u128` smallest units (for KEEL: orcas, `keel_types::ORCAS_PER_KEEL`
  to the KEEL); all rounding floors; use
  `keel_types::mul_div_floor`.
- Ids are counters in state, never hashes of wall-clock data.
- Money moves only through `state.ledger.post(...)`; escrow is a transfer
  into a restricted account. Fees go through `modules::fees::collect`.
- Atomicity: validate, then post to the ledger, then mutate module state.
  On any error return early; `apply.rs` rolls the ledger back.

## Snapshots and upgrades
- A snapshot is `b"KEEL" | u32 schema | borsh(State)` (`keel-vm/src/migrate.rs`).
  Before mainnet there is no migration path: a node loads a snapshot of its
  own schema only. Changing the layout of `State` or anything inside it
  means bumping `SCHEMA` (so no node misreads a foreign snapshot) and
  restarting the networks from genesis with `reset_chain`; the only fixture
  under `keel-vm/tests/fixtures/` is the current schema's, rewritten by
  `cargo test -p keel-vm --test migrate` after a bump. Frozen layouts,
  "off when migrated" defaults and compatibility stubs are not written.
- The same holds for behaviour: a change to what a block does ships as is,
  with no activation-height gate, and the networks restart. The
  `SoftwareUpgrade` proposal kind and `gov::feature_active` exist for
  mainnet, where a behaviour change must keep the old path below its
  activation height so replaying the journal from genesis stays
  byte-identical on every node.

## End to end
- `infra/dev/*-e2e.sh` are the integration checks; `NO_BUILD=1` reuses the
  release binaries, `DEVNET_IDLE_MS=1000` makes idle blocks fast. They share
  `devnet.sh stop` (which kills every local `keel-node`), so run one at a
  time. A change to the node, the observer or the signer is not done until
  `e2e.sh`, `custody-e2e.sh`, `sync-e2e.sh` and `failover-e2e.sh` pass
  locally; a change to signing or custody also needs `tss-e2e.sh` and
  `client-vault-e2e.sh` (a client-owned vault with its own HTTP signer).

## Parameters and module state
- `Params` keeps its layout: a new knob for a module goes into that module's
  own state (`state.clients.params` is the pattern) and is reached through
  `ParamChange` with a dotted key (`clients.usage_address_usd_micro`),
  routed in `gov::set_param`. New module states are appended at the end of
  `State`, which is a schema bump (see above).
- No numeric limit is a constant in a module. A cap, a floor, a window or a
  price that an action is checked against lives in `state.limits` (or the
  module's own params) with a key in `limits::PARAM_KEYS`, bounds in
  `limits::set_param`, and a row in `docs/governance.md`. The only literals
  left are protocol constants (block capacity, unit scales) and node-local
  tuning in `keel-node`.

## Build and test
- Use your own target dir: `CARGO_TARGET_DIR=$PWD/target-<name> cargo test -p <crate>`.
- Run `cargo clippy -p <crate> --all-targets` before finishing; the
  deny-lists in `clippy.toml` are mandatory.
- Every module ships tests under `crates/keel-vm/tests/<name>.rs` (or the
  crate's own tests) that drive `apply_block` end to end and assert
  `state.ledger.audit().mismatches.is_empty()`.
