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

## Setting up the GitHub environment (step by step)

Done once per box. Everything below uses the GitHub CLI logged in as an
org owner (`gh auth status`); the web UI works the same way under
*Settings → Environments → testnet*. Secrets are piped into `gh`, never
pasted into a terminal or a chat.

1. **Repository and environment.** The repo is `keelchain/chain`. Create the
   environment and allow deployments from `main` only:

   ```bash
   R=keelchain/chain; E=testnet
   gh api -X PUT repos/$R/environments/$E --input - <<'EOT'
   {"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
   EOT
   gh api -X POST repos/$R/environments/$E/deployment-branch-policies -f name=main -f type=branch
   ```

2. **Deploy key.** A dedicated SSH key for the workflow; the public half goes
   into the deploy user's `authorized_keys` on the box, the private half into
   the environment, and the local file is shredded:

   ```bash
   ssh-keygen -t ed25519 -N "" -C keel-testnet-deploy -f deploy_key
   gh secret set SSH_PRIVATE_KEY -R $R --env $E < deploy_key
   ssh ubuntu@<box ip> 'cat >> ~/.ssh/authorized_keys' < deploy_key.pub
   ssh -i deploy_key -o IdentitiesOnly=yes ubuntu@<box ip> 'sudo -n true && echo ok'
   shred -u deploy_key
   ssh-keyscan -t ed25519,rsa <box ip> | gh variable set DEPLOY_KNOWN_HOSTS -R $R --env $E
   ```

3. **Chain seeds.** Generated, never displayed. Changing either one later
   means a new chain (`reset_chain`):

   ```bash
   python3 -c 'import secrets; print(secrets.randbits(63) | (1 << 62))' | gh secret set KEEL_VALIDATOR_SEED -R $R --env $E
   openssl rand -hex 32 | gh secret set KEEL_SIGNER_SEED -R $R --env $E
   ```

4. **Box credentials.** Read on the box and piped straight into GitHub:

   ```bash
   ssh ubuntu@<box ip> "docker inspect <pg container> --format '{{range .Config.Env}}{{println .}}{{end}}' | sed -n 's/^POSTGRES_PASSWORD=//p'" \
     | python3 -c 'import sys; print("postgres://<pg user>:%s@127.0.0.1:<pg port>/keel_indexer_testnet" % sys.stdin.read().strip())' \
     | gh secret set INDEXER_DATABASE_URL -R $R --env $E
   ssh ubuntu@<box ip> "sudo sed -n 's/^rpcpassword=//p' /var/lib/bitcoin-signet/bitcoin.conf" | gh secret set BITCOIN_RPC_PASSWORD -R $R --env $E
   ```

   `TRONGRID_API_KEY` is optional; set it the same way when there is one.

5. **Variables.** Plain settings, one call each (`gh variable set NAME -R $R
   --env $E --body VALUE`). An empty value makes `gh` wait on stdin, so leave
   optional ones such as `KEEL_BOOTSTRAP_ARGS` unset instead:

   | name | value on the current box |
   |---|---|
   | `DEPLOY_HOST` / `DEPLOY_USER` | the box IP / `ubuntu` |
   | `KEEL_CHAIN_ID` / `KEEL_ADVERTISE` / `KEEL_EXTERNAL_NETWORK` | `3` / `<box ip>:3000` / `signet` |
   | `BITCOIN_RPC_URL` / `BITCOIN_RPC_USER` / `BITCOIN_WALLET` | `http://127.0.0.1:38332` / the bitcoind RPC user / `keel-testnet-vault` |
   | `BITCOIN_CLI` | `<path to bitcoin-cli> -datadir=/var/lib/bitcoin-signet -conf=/var/lib/bitcoin-signet/bitcoin.conf` |
   | `INDEXER_PG_CONTAINER` | the Postgres container name |
   | `TRON_API_URL` / `TRON_USDT_CONTRACT` | `https://nile.trongrid.io` / the Nile USDT contract |
   | `EXPLORER_NETWORKS` | `[{"id":"testnet","name":"Keel testnet","api":"https://testnet.keelchain.com/api"}]` |

6. **Check and deploy.**

   ```bash
   gh secret list -R $R --env $E      # 5 names, values never shown
   gh variable list -R $R --env $E
   gh workflow run deploy-testnet.yml -R $R --ref main -f reset_chain=true   # first run, or a new chain
   gh run watch -R $R
   ```

   A push to `main` runs the same deploy without a reset. If the box has no
   genesis yet, the deploy bootstraps one on its own.

## Rotating

- **Deploy key:** repeat step 2; remove the old line from `authorized_keys`.
- **Seeds:** repeat step 3, then run the deploy with `reset_chain`. The old
  chain, its balances and roles are gone; onboard clients again.
- **Database or Bitcoin password:** change it on the box, repeat step 4, push
  or dispatch a deploy; the units restart with the new config.
