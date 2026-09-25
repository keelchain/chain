# Keelchain

Keelchain, a Layer-1 settlement chain for peer-to-peer trading: balances, order books, P2P fiat
offers, escrow, disputes, cross-chain custody, a USD stablecoin, staking and
governance in one deterministic Rust state machine ordered by Simplex BFT.
Design: `../docs/plan.md` (start there), `whitepaper.md`,
`tokenomics.md`, `migration.md`; working rules: `dev-rules.md`.

## Layout

| Crate | Role |
|---|---|
| `keel-types`, `keel-crypto` | integer money math, assets, addresses, hashing, ed25519 |
| `keel-ledger` | double-entry ledger (restricted accounts, idempotent postings, savepoints) |
| `keel-book` | pure price-time matcher with a synthetic house level |
| `keel-actions` | signed action envelope, the full action set, per-address budgets, block payload codec |
| `keel-vm` | the state machine: `apply_block` over modules tokens / markets / budgets / p2p / disputes / vaults / stable / staking / gov / attest, block-boundary invariants |
| `keel-lc-btc`, `keel-lc-eth`, `keel-attest` | Bitcoin SPV and Ethereum sync-committee proof verification, observer attestation quorum |
| `keel-consensus` | Commonware simplex adapter, marshal, one engine per epoch with validator sets from staking |
| `keel-rpc`, `keel-node`, `keel-cli` | HTTP/WS API, the validator binary (mempool, gossip, snapshots, receipts), the `keel` CLI (incl. `genesis-from-export`) |
| `keel-chains`, `keel-tss`, `keel-observer` | HD derivation and BTC/ETH/TRON tx builders, cggmp21 threshold signer daemon, observer-signer daemon |
| `keel-indexer` | block explorer backend: follows a node, full history in Postgres, serves `docs/explorer-api.md` for `../apps/explorer` (one process per network) |

`../sdk/ts` is the TypeScript SDK (borsh codec byte-identical to `keel-actions`).

## Build and test

Rust 1.95+ (pinned in `rust-toolchain.toml`). `keel-tss` builds GMP from
source and needs `m4` on the PATH (`~/.local/bin` if installed without sudo).

```sh
cargo test --workspace            # ~150 tests, all crates
cargo clippy --workspace --all-targets
../infra/dev/devnet.sh --devnet # 4 validators on localhost, RPC on 5000..5003
../infra/dev/e2e.sh             # devnet + crossing orders + restart catch-up
../infra/dev/custody-e2e.sh     # + regtest bitcoind deposit/withdraw through observers
../infra/dev/explorer.sh start  # indexer on :6100 + explorer on :5177 against the devnet
```

Indexer alone (needs Postgres; migrations run at start):

```sh
keel-indexer --network testnet --node-rpc http://127.0.0.1:5000 \
  --database-url postgres://keel:keel@localhost:5434/keel_indexer_testnet --listen 127.0.0.1:6100
```

For mainnet run a second instance with `--network mainnet`, its own
database and `--listen 127.0.0.1:6101`; the explorer lists both through
`VITE_NETWORKS`.

Determinism rules for everything at `keel-vm` and below are enforced by
`clippy.toml` (no floats, no `HashMap`, no wall clock).
