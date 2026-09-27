# Working rules for the Keelchain workspace

Read `plan.md` first. These rules keep parallel work from colliding.

## Ownership
- One crate or one `keel-vm/src/modules/<name>.rs` per workstream. Do not edit
  another workstream's files.
- Shared files (`keel-vm/src/{state,apply,receipt,params,genesis}.rs`,
  `keel-actions/src/*`, root `Cargo.toml`): use the **Edit** tool with a small,
  unique anchor; never rewrite them wholesale. Add new `Event` variants and
  `Params` fields at the END of their lists. Never rename or remove an
  existing item.
- New external crates: add to your own crate's `[dependencies]` with an
  explicit version. Do not touch `[workspace.dependencies]`.

## Determinism (everything at `keel-vm` and below)
- No floats, no `HashMap`/`HashSet`, no wall clock, no randomness. Block time
  is `BlockContext::timestamp` (ms) / `seconds()`.
- Amounts are `u128` smallest units; all rounding floors; use
  `keel_types::mul_div_floor`.
- Ids are counters in state, never hashes of wall-clock data.
- Money moves only through `state.ledger.post(...)`; escrow is a transfer
  into a restricted account. Fees go through `modules::fees::collect`.
- Atomicity: validate, then post to the ledger, then mutate module state.
  On any error return early; `apply.rs` rolls the ledger back.

## Snapshots and upgrades
- A snapshot is `b"KEEL" | u32 schema | borsh(State)` (`keel-vm/src/migrate.rs`).
  History: schema 1 added the header, 2 appended `State.clients`, 3 appended
  `State.custody`; every older layout is a frozen `StateVn` in `migrate.rs`.
  Changing the layout of `State` or anything inside it means: bump `SCHEMA`,
  keep a frozen copy of the old struct in `migrate.rs`, add the upgrade step,
  and add a fixture of the old schema under `keel-vm/tests/fixtures/` with a
  case in `tests/migrate.rs`. `cargo test -p keel-vm --test migrate` must keep
  loading every fixture.
- A change that alters what a block does (fees, matching, limits, new actions
  with new effects) is gated on an activation height: read it from
  `state.gov.upgrades` (the `SoftwareUpgrade` proposal kind) and keep the old
  behaviour below it, so replaying the journal from genesis stays
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
  `SetParam` / `ParamChange` with a dotted key (`clients.usage_address_keel`),
  routed in `gov::set_param`. New module states are appended at the end of
  `State`, which is a schema bump (see above).

## Build and test
- Use your own target dir: `CARGO_TARGET_DIR=$PWD/target-<name> cargo test -p <crate>`.
- Run `cargo clippy -p <crate> --all-targets` before finishing; the
  deny-lists in `clippy.toml` are mandatory.
- Every module ships tests under `crates/keel-vm/tests/<name>.rs` (or the
  crate's own tests) that drive `apply_block` end to end and assert
  `state.ledger.audit().mismatches.is_empty()`.
