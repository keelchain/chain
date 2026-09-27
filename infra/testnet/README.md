# Testnet deployment

Everything on the testnet hosts is installed by `.github/workflows/deploy-testnet.yml`
through `deploy-remote.sh`. Nothing is edited on a box by hand; to change a
secret or a setting, change the GitHub environment and rerun the workflow.

The testnet is a set of hosts. Every host runs a validator, an observer and a
threshold signer; host 0 also runs
the indexer, the web roots behind nginx, the shared signet `bitcoind` and the
checkpoint proposer. The hosts talk over a WireGuard mesh (`keel` interface,
`10.90.0.0/24`) for the signer protocol and Bitcoin RPC; consensus p2p is
public on port 3000.

**How many hosts.** Consensus is BFT: `n = 3f + 1`. One validator tolerates
nothing, three tolerate nothing either (one down halts the chain), four
tolerate one fault. Custody is separate: the vault key is `t`-of-`n` across
the observers, so 2-of-3 signers keep withdrawing with one host down even
while consensus needs all three. For a testnet that stays up through one
box failing, deploy four hosts (four small boxes, or the fourth validator as
a second process on host 0, which covers a crash but not that box going
down). The workflows take any number of hosts.

## GitHub environment `testnet`

Secrets:

| name | what |
|---|---|
| `SSH_PRIVATE_KEY` | deploy key accepted by every host's deploy user (passwordless sudo) |
| `KEEL_VALIDATOR_SEED_<i>` | decimal u64 per host: the host's consensus key and its validator/observer account. Changing one means a new chain (`reset_chain`). |
| `WG_PRIVATE_KEY_<i>` | the host's WireGuard private key (`prepare-host.sh` creates it in `/etc/wireguard/keel.key`) |
| `KEEL_TSS_PASSPHRASE` | encrypts every host's share of the vault key |
| `KEEL_TSS_SECRET` | 64 hex; HMAC key of the signer set's transport |
| `KEEL_SIGNER_SEED` | 64 hex; only with `KEEL_SIGNER_MODE=local` (single-host development signer) |
| `KEEL_API_KEYS` | optional; comma-separated `label:token` API keys. When set, `POST /rpc/v1/actions` needs `Authorization: Bearer <token>`; reads stay open. Generate tokens with `openssl rand -hex 24` and hand each client its own |
| `INDEXER_DATABASE_URL` | Postgres URL for the indexer on host 0 |
| `BITCOIN_RPC_PASSWORD` | the signet bitcoind RPC password (host 0's `bitcoin.conf`) |
| `TRONGRID_API_KEY` | optional |
| `RCLONE_CONF` | optional; an rclone config defining the `keel-backups` remote (B2, S3, …) the daily backup copies to. Without it archives stay under `/var/backups/keel` |
| `ALERT_SMTP_PASSWORD`, `GRAFANA_PASSWORD` | optional; with `KEEL_MONITORING=1` |

Variables:

| name | value |
|---|---|
| `DEPLOY_HOSTS` | JSON list, host order = index: `[{"ip":"<public ip>","user":"ubuntu","wg_ip":"10.90.0.1"},{"ip":"…","wg_ip":"10.90.0.2"},{"ip":"…","wg_ip":"10.90.0.3"}]` |
| `DEPLOY_HOST_INDEXES` | `[0,1,2]` (which hosts the matrix deploys) |
| `DEPLOY_HOST`, `DEPLOY_USER` | the old single-box layout, still honoured as host 0 when `DEPLOY_HOSTS` is unset (with the old `KEEL_VALIDATOR_SEED` secret standing in for `KEEL_VALIDATOR_SEED_0`) |
| `WG_PUBKEYS` | JSON list of the hosts' WireGuard public keys, host order |
| `DEPLOY_KNOWN_HOSTS` | optional `ssh-keyscan` output for every host; without it host keys are scanned at deploy time |
| `KEEL_CHAIN_ID` | `3` |
| `KEEL_EXTERNAL_NETWORK` | `signet` |
| `KEEL_SIGNER_MODE` | `tss` (threshold signer across the hosts) or `local` (one host, development signer) |
| `KEEL_OBSERVER_THRESHOLD` | optional; default two thirds of the hosts, rounded up |
| `KEEL_VALIDATOR_BOND` | optional; KEEL bonded per validator at genesis (smallest units, default 100,000 KEEL) |
| `KEEL_PARAM_ADMIN`, `KEEL_ATTESTER`, `KEEL_ARBITRATOR` | 64-hex addresses of keys I hold off the boxes; default to host 0's account when unset |
| `KEEL_GENESIS_FUNDS` | optional extra credits, comma separated `<address>:<ASSET>:<amount>` |
| `KEEL_NODE_EXTRA_ARGS` | optional extra `keel-node` flags on every host, e.g. `--extra-peers <follower key>` |
| `BITCOIN_RPC_URL`, `BITCOIN_RPC_USER`, `BITCOIN_WALLET` | `http://127.0.0.1:38332` (host 0), `keel`, `keel-vault-watch` (suffixed with the host index) |
| `INDEXER_PG_CONTAINER` | the Postgres container the indexer database lives in (recreated on reset) |
| `TRON_API_URL`, `TRON_USDT_CONTRACT` | `https://nile.trongrid.io`, the Nile USDT contract |
| `EXPLORER_NETWORKS` | JSON network list baked into the explorer build |
| `AGE_RECIPIENT` | optional; an `age` public key; backups are encrypted to it (keep the identity off the boxes) |
| `KEEL_CLIENTS` | optional; JSON list of clients to re-onboard after a reset and to pre-approve in governance votes: `[{"address":"<hex>","attester":true,"param_admin":false,"keel":100000,"kusd":100000}]` |
| `KEEL_MONITORING` | `1` to run Prometheus, Alertmanager and Grafana on host 0 (`infra/testnet/monitoring`), scraping every host's node_exporter over the mesh |
| `ALERT_SMTP_HOST`, `ALERT_SMTP_FROM`, `ALERT_SMTP_USER`, `ALERT_TO` | where alerts are mailed |

## What a deploy does

1. `binaries`: builds `keel-node`, `keel-indexer`, `keel-observer`, `keel`, `keel-tss`.
2. `web`: builds the explorer, the site, the docs pages and the wallet packages.
3. `genesis`: derives every host's public identity from its seed
   (`keel-node --print-identity`), writes `identities.json`, and builds
   `genesis.json` with `keel genesis-build`: the hosts as validators and
   observers (threshold from `KEEL_OBSERVER_THRESHOLD`), the arbitrator,
   attester and param admin from the variables, an operating float of KEEL
   for every validator account and KUSD for host 0, `genesis-params.json`
   applied. The file is deterministic; a deploy without `reset_chain` refuses
   to run when it differs from the installed genesis.
4. `deploy` (one job per host): installs the binaries, renders the WireGuard
   config, the node, observer, signer and checkpoint configs under
   `/etc/keelchain/` (root-only), installs the units and restarts the node.
   On a reset it wipes the chain state first and, on host 0, recreates the
   indexer database. Host 0 also gets the web roots and the nginx config
   (when the Cloudflare origin certificate is present).
5. `checks`: the public endpoints answer.

In `tss` mode a fresh chain has no vault key yet: run the **TSS ceremony**
workflow next. It runs the distributed key generation on every host in
parallel (each host keeps only its share, encrypted under
`KEEL_TSS_PASSPHRASE`), checks that every host derived the same public key,
registers the vaults for the requested chains from host 0 with the hosts'
observer accounts as signers, and proposes the first Bitcoin checkpoint. In
`local` mode the deploy registers the vaults itself with the development
signer.

Bitcoin checkpoints go through governance: host 0 proposes daily
(`keel-btc-checkpoint.timer`), every host votes for proposals whose header
matches its own view of Bitcoin (`keel-vote.timer`, every ten minutes), and
host 0 executes after the timelock. Two of three validators
carry a proposal.

## Operations

- **Backups.** `keel-backup.timer` runs `backup.sh` daily on every host: the
  newest snapshot with its sidecar and epoch boundaries, the genesis, the
  root-only config, and on host 0 a `pg_dump` of the indexer. Encrypted to
  `AGE_RECIPIENT`, copied to the `keel-backups` rclone remote, 14 days kept
  locally and remotely. `restore.sh <archive> [age identity]` puts a host
  back on that snapshot (the deploy re-renders the config). Drill it on the
  staging box before trusting it.
- **Monitoring.** `keel-probe.timer` writes `keel_*` metrics every minute
  for node_exporter (height, oldest block, readiness and checkpoint age per
  network, pending outbounds, unit states and restarts). With
  `KEEL_MONITORING=1` host 0 runs Prometheus, Alertmanager and Grafana from
  `monitoring/`; `alerts.yml` pages on a stalled chain, a node down, a unit
  down or flapping, a stale checkpoint, stuck outbounds, a filling disk or a
  silent probe. Without the stack, point Grafana Cloud's agent at the same
  node_exporter targets. `soak-report.sh [days]` summarises a soak from
  Prometheus.
- **Votes.** `keel-vote.timer` votes Yes every ten minutes on proposals I
  pre-approved on the host: Bitcoin checkpoints that match its own view,
  attester and param-admin proposals for addresses in `KEEL_CLIENTS`, and
  software upgrades whose version the propose-upgrade run wrote to
  `/etc/keelchain/upgrades-approved`. Everything else waits for a hand vote.
- **Upgrades.** `keel-node` compares its version with the executed
  `SoftwareUpgrade` proposals before every block and stops at an activation
  height it was not voted in for. The `Propose upgrade` workflow proposes,
  gets the hosts' votes, waits for the timelock and executes; then push the
  release so the deploy installs the matching binary before the height.
- **Staging.** The deploy takes an `environment` input: a `testnet-staging`
  environment with one small box and `KEEL_SIGNER_MODE=local` runs the same
  workflows for a rehearsal (restore drill, upgrade drill) before the
  testnet. Give the `testnet` environment a required reviewer in GitHub so a
  push cannot replace the live binaries unreviewed.
- **Resets.** A reset wipes roles and balances; the deploy re-onboards every
  client in `KEEL_CLIENTS` (host 0 proposes and votes, the other hosts' vote
  timers carry the proposals) and the changelog announces resets ahead.

## Preparing a host

Once per box, as a sudoer over SSH (Ubuntu 24.04):

```bash
scp infra/testnet/prepare-host.sh ubuntu@<ip>:
ssh ubuntu@<ip> 'bash prepare-host.sh <index> 10.90.0.<index+1>'
```

It installs the packages, creates the WireGuard key and prints its public
key, sets the firewall (ssh, p2p, WireGuard from anywhere; signer and
Bitcoin RPC only over the mesh; https on host 0), and on host 0 creates the
indexer's Postgres container, the signet `bitcoin.conf` and the `bitcoind`
unit. Bitcoin Core itself and the Cloudflare origin certificate
(`/etc/ssl/keelchain/origin.{pem,key}`) are installed by hand on host 0.

## Setting up the GitHub environment (step by step)

Everything below uses the GitHub CLI logged in as an org owner; the web UI
works the same way under *Settings → Environments → testnet*. Secrets are
piped into `gh`, never pasted into a terminal or a chat.

1. **Repository and environment.** The repo is `keelchain/chain`. Create the
   environment and allow deployments from `main` only:

   ```bash
   R=keelchain/chain; E=testnet
   gh api -X PUT repos/$R/environments/$E --input - <<'EOT'
   {"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
   EOT
   gh api -X POST repos/$R/environments/$E/deployment-branch-policies -f name=main -f type=branch
   ```

2. **Deploy key.** One SSH key for the workflow; the public half goes into
   every host's `authorized_keys`, the private half into the environment:

   ```bash
   ssh-keygen -t ed25519 -N "" -C keel-testnet-deploy -f deploy_key
   gh secret set SSH_PRIVATE_KEY -R $R --env $E < deploy_key
   for ip in <ip0> <ip1> <ip2>; do ssh ubuntu@$ip 'cat >> ~/.ssh/authorized_keys' < deploy_key.pub; done
   shred -u deploy_key
   { for ip in <ip0> <ip1> <ip2>; do ssh-keyscan -t ed25519 $ip; done; } | gh variable set DEPLOY_KNOWN_HOSTS -R $R --env $E
   ```

3. **Hosts.** After `prepare-host.sh` on each box:

   ```bash
   gh variable set DEPLOY_HOSTS -R $R --env $E --body '[{"ip":"<ip0>","user":"ubuntu","wg_ip":"10.90.0.1"},{"ip":"<ip1>","user":"ubuntu","wg_ip":"10.90.0.2"},{"ip":"<ip2>","user":"ubuntu","wg_ip":"10.90.0.3"}]'
   gh variable set DEPLOY_HOST_INDEXES -R $R --env $E --body '[0,1,2]'
   gh variable set WG_PUBKEYS -R $R --env $E --body '["<pub0>","<pub1>","<pub2>"]'
   for i in 0 1 2; do ssh ubuntu@<ip$i> 'sudo cat /etc/wireguard/keel.key' | gh secret set WG_PRIVATE_KEY_$i -R $R --env $E; done
   ```

4. **Chain seeds and signer secrets.** Generated, never displayed. Changing a
   validator seed later means a new chain (`reset_chain`); changing the TSS
   passphrase or secret means a new ceremony:

   ```bash
   for i in 0 1 2; do python3 -c 'import secrets; print(secrets.randbits(63) | (1 << 62))' | gh secret set KEEL_VALIDATOR_SEED_$i -R $R --env $E; done
   openssl rand -base64 32 | gh secret set KEEL_TSS_PASSPHRASE -R $R --env $E
   openssl rand -hex 32   | gh secret set KEEL_TSS_SECRET -R $R --env $E
   gh variable set KEEL_SIGNER_MODE -R $R --env $E --body tss
   ```

5. **Roles I hold off the boxes.** Generate three keys locally, keep the
   secrets in a password manager, and publish only the addresses:

   ```bash
   keel keygen   # three times; keep each "secret", set each "address":
   gh variable set KEEL_PARAM_ADMIN -R $R --env $E --body <address>
   gh variable set KEEL_ATTESTER    -R $R --env $E --body <address>
   gh variable set KEEL_ARBITRATOR  -R $R --env $E --body <address>
   gh variable set KEEL_GENESIS_FUNDS -R $R --env $E --body '<param admin address>:KEEL:1000000000000,<param admin address>:KUSD:1000000000000'
   ```

6. **Host 0 credentials.** Read on the box and piped straight into GitHub:

   ```bash
   ssh ubuntu@<ip0> "sudo docker inspect keel-pg --format '{{range .Config.Env}}{{println .}}{{end}}' | sed -n 's/^POSTGRES_PASSWORD=//p'" \
     | python3 -c 'import sys; print("postgres://keel:%s@127.0.0.1:5434/keel_indexer_testnet" % sys.stdin.read().strip())' \
     | gh secret set INDEXER_DATABASE_URL -R $R --env $E
   ssh ubuntu@<ip0> "sudo sed -n 's/^rpcpassword=//p' /var/lib/bitcoin-signet/bitcoin.conf" | gh secret set BITCOIN_RPC_PASSWORD -R $R --env $E
   gh variable set INDEXER_PG_CONTAINER -R $R --env $E --body keel-pg
   ```

   `TRONGRID_API_KEY` is optional; set it the same way when there is one.

7. **Plain variables.** `gh variable set NAME -R $R --env $E --body VALUE`
   for `KEEL_CHAIN_ID` (`3`), `KEEL_EXTERNAL_NETWORK` (`signet`),
   `BITCOIN_RPC_URL` (`http://127.0.0.1:38332`), `BITCOIN_RPC_USER` (`keel`),
   `BITCOIN_WALLET` (`keel-vault-watch`), `TRON_API_URL`, `TRON_USDT_CONTRACT`,
   `EXPLORER_NETWORKS`. An empty value makes `gh` wait on stdin, so leave
   optional ones unset instead.

8. **Deploy, then the ceremony.**

   ```bash
   gh workflow run deploy-testnet.yml -R $R --ref main -f reset_chain=true   # first run, or a new chain
   gh run watch -R $R
   gh workflow run tss-ceremony.yml -R $R --ref main -f epoch=1 -f chains=BTC,TRON
   gh run watch -R $R
   ```

   A push to `main` runs the same deploy without a reset. If a host has no
   genesis yet, the deploy installs the release genesis on it.

## One host only

The same workflows run a single box: `DEPLOY_HOSTS` with one entry (no
`wg_ip` needed), `DEPLOY_HOST_INDEXES` `[0]`, `KEEL_SIGNER_MODE` `local` with
`KEEL_SIGNER_SEED` set, no WireGuard secrets. The genesis then has one
validator and one observer with threshold 1, and the deploy registers the
vaults with the development signer as before.

## Onboarding a client

`.github/workflows/onboard-client.yml` runs `onboard-client.sh` on host 0:
attester and param-admin roles through governance (propose with host 0's
validator account, vote, wait for the timelock, execute) and KEEL / KUSD
transfers from that account's operating float. With three validators the
other hosts' vote timers do not cover client proposals yet, so run the
workflow while the proposal is open and vote from the other hosts'
accounts by hand, or lower `quorum_bps` on the testnet. The current role
sets are public at `/rpc/v1/gov/roles`.

## Rotating

- **Deploy key:** repeat step 2; remove the old line from `authorized_keys`.
- **Validator seeds:** repeat step 4 for the seed, then run the deploy with
  `reset_chain`. The old chain, its balances and roles are gone; onboard
  clients again.
- **Vault key:** run the ceremony with a higher `epoch`; funds in the old
  epoch's addresses stay spendable by the old shares (`share.enc.prev`)
  until they are swept; moving them is a manual withdrawal from the old
  vault.
- **Database or Bitcoin password:** change it on host 0, repeat step 6, push
  or dispatch a deploy; the units restart with the new config.
