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

## Build and test
- Use your own target dir: `CARGO_TARGET_DIR=$PWD/target-<name> cargo test -p <crate>`.
- Run `cargo clippy -p <crate> --all-targets` before finishing; the
  deny-lists in `clippy.toml` are mandatory.
- Every module ships tests under `crates/keel-vm/tests/<name>.rs` (or the
  crate's own tests) that drive `apply_block` end to end and assert
  `state.ledger.audit().mismatches.is_empty()`.
