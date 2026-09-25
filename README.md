<p align="center"><img src="brand/keel-logo.svg" alt="Keelchain" height="48"></p>

# Keelchain

A settlement chain for peer-to-peer trading: on-chain escrow with arbitrated
disputes, threshold-signed vaults for Bitcoin, Ethereum and Tron, the `KUSD`
stablecoin backed one to one by vaulted USDT, and a free action budget instead
of gas. Public testnet at https://testnet.keelchain.com, site and wallet at
https://keelchain.com. Front ends are clients of the chain; anyone can build one
on the same API.

```
chain/                  Rust workspace: keel-node, keel-indexer, keel-observer, keel (CLI), keel-tss, libraries
sdk/ts/                 @keelchain/sdk: action encoding, Ed25519 signing, RPC client (TypeScript)
apps/explorer/          block explorer (Vite + React) over the indexer API
apps/wallet-extension/  Keel Wallet, MV3 browser extension exposing window.keel
site/                   keelchain.com (static)
brand/                  logo, mark, colours
infra/testnet/          deploy script, systemd units, nginx, genesis overrides, onboarding
infra/dev/              local devnet, e2e scripts (regtest bitcoind, LND, Tron mocks)
docs/                   how-it-works, whitepaper, tokenomics, wallet, explorer API, testnet
```

## Build and test

```
cd chain && cargo build --release && cargo test --workspace
cd sdk/ts && npm ci && npm run build && npm test
cd apps/explorer && npm ci && npm test && npm run build
cd apps/wallet-extension && npm ci && npm test && npm run package
```

Rust 1.98 and `m4` (GMP for the threshold-signing crate). A local devnet:
`infra/dev/devnet.sh`; the full custody loop against regtest Bitcoin:
`infra/dev/custody-e2e.sh`.

## Testnet

The testnet is deployed only through GitHub Actions
(`.github/workflows/deploy-testnet.yml`): every push to `main` builds the
binaries, the explorer, the site and the wallet packages and installs them on
the box. Secrets and settings live in the `testnet` GitHub environment;
`infra/testnet/README.md` lists them and `docs/testnet.md` describes what runs.
`onboard-client.yml` gives a client account its roles and starting balances.

## Read next

- `docs/how-it-works.md`: the chain end to end, module by module.
- `docs/whitepaper.md` and `docs/tokenomics.md`: why, and the KEEL model.
- `docs/explorer-api.md`: the read API third parties build on.
- `docs/wallet.md`: the wallet's provider API and key model.
- `docs/launch.md`: the brand, hosting and the plan for third-party clients.

Contact: mohab@keelchain.com (partnerships, API access, roles),
support@keelchain.com, wallet@keelchain.com.
