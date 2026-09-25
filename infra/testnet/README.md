# Testnet deployment

Everything on the testnet box is installed by `.github/workflows/deploy-testnet.yml`
through `deploy-remote.sh`. Nothing is edited on the box by hand; to change a
secret or a setting, change the GitHub environment and rerun the workflow.

## GitHub environment `testnet`

Secrets:

| name | what |
|---|---|
| `SSH_PRIVATE_KEY` | key for `DEPLOY_USER@DEPLOY_HOST` (passwordless sudo) |
| `KEEL_VALIDATOR_SEED` | decimal u64; the validator's identity and, on a one-validator testnet, the genesis account, observer, arbitrator, attester and param admin. Changing it means a new chain (`reset_chain`). |
| `KEEL_SIGNER_SEED` | 64 hex; the development signer behind the vault key (`tss_url = local:`). Changing it needs new vault epochs. |
| `INDEXER_DATABASE_URL` | Postgres URL for the indexer, e.g. `postgres://keel:…@127.0.0.1:5434/keel_indexer_testnet` |
| `BITCOIN_RPC_PASSWORD` | the signet bitcoind RPC password |
| `TRONGRID_API_KEY` | optional |

Variables:

| name | example |
|---|---|
| `DEPLOY_HOST`, `DEPLOY_USER` | `<box ip>`, `<deploy user>` |
| `DEPLOY_KNOWN_HOSTS` | optional `ssh-keyscan` output; without it the host key is scanned at deploy time |
| `KEEL_CHAIN_ID` | `3` |
| `KEEL_ADVERTISE` | `<box ip>:3000` |
| `KEEL_EXTERNAL_NETWORK` | `signet` |
| `KEEL_BOOTSTRAP_ARGS` | empty on a one-validator net, else `--bootstrappers <pubkey>@<ip:port>` |
| `BITCOIN_RPC_URL`, `BITCOIN_RPC_USER`, `BITCOIN_WALLET` | `http://127.0.0.1:38332`, `keel`, `keel-vault-watch-0` |
| `BITCOIN_CLI` | `/usr/local/bin/bitcoin-cli -datadir=/var/lib/bitcoin-signet -conf=/var/lib/bitcoin-signet/bitcoin.conf` |
| `INDEXER_PG_CONTAINER` | `<container name>`: the Postgres container the indexer database lives in (recreated on reset) |
| `TRON_API_URL`, `TRON_USDT_CONTRACT` | `https://nile.trongrid.io`, the Nile USDT contract |
| `EXPLORER_NETWORKS` | JSON network list baked into the explorer build |

## What the deploy does

1. Installs `keel-node`, `keel-indexer`, `keel-observer`, `keel` to `/opt/keelchain/bin`.
2. Derives the validator account from the seed and writes `/etc/keelchain/`
   (`validator-0.env`, `observer-0.env`, `observer-0.toml`, `indexer.env`,
   `checkpoint.env`, `pubkey0`), root-only.
3. If `reset_chain` was requested or no genesis exists: stops the units, wipes
   `/var/lib/keelchain`, prints a devnet genesis for the seed, applies
   `genesis-params.json`, recreates the indexer database.
4. Installs the systemd units, starts the validator, registers the BTC and
   TRON vaults with the development signer on a fresh chain, starts the
   indexer and the observer, enables the daily Bitcoin checkpoint timer.
5. Copies the explorer build to `/var/www/keelchain/explorer` and `site/` (with
   the wallet packages under `wallet/downloads/`) to `/var/www/keelchain/site`.
6. Installs `nginx-keelchain.conf` when the Cloudflare origin certificate is
   present at `/etc/ssl/keelchain/origin.{pem,key}`, and reloads nginx.

Prerequisites on the box that the deploy does not manage: Ubuntu 24.04 with
passwordless sudo for the deploy user, Docker with a Postgres container for the
indexer, a pruned signet `bitcoind` with RPC on loopback, nginx, and the
Cloudflare origin certificate. Cloudflare: proxied A records for
`keelchain.com` and `testnet.keelchain.com`, TLS mode Full (strict).

## Onboarding a client

`.github/workflows/onboard-client.yml` runs `onboard-client.sh` on the box:
attester and param-admin roles through governance (propose, vote with the
genesis validator, wait for the timelock, execute) and KEEL / KUSD transfers.
The current role sets are public at `/rpc/v1/gov/roles`.
