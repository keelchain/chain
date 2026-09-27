# The public testnet

Live since 2026-09-24 at keelchain.com, deployed only by GitHub Actions
(`infra/testnet/README.md`).

| Piece | Where |
|---|---|
| site | https://keelchain.com (`site/`, wallet pages, wallet packages under `/wallet/downloads/`) |
| explorer | https://testnet.keelchain.com (`apps/explorer`, `VITE_BASE=/`) |
| read API | https://testnet.keelchain.com/api/v1/… (`keel-indexer`, `docs/explorer-api.md`) |
| node RPC | https://testnet.keelchain.com/rpc/v1/… (`keel-node`; status, accounts, params, `gov/roles`, `gov/proposals`, receipts, WebSocket at `/rpc/v1/ws`) |
| chain | id 3, `_KEEL_CHAIN` namespace, external network signet; the validator set is the host set in `DEPLOY_HOSTS` (one host today; consensus is BFT, so four validators tolerate one fault and three tolerate none), 600-block epochs; genesis built by the deploy workflow from every host's public identity plus `infra/testnet/genesis-params.json` (voting 60 blocks, timelock 10, proposal deposit 1000 KEEL) |
| vaults | BTC (signet, pruned bitcoind on host 0, shared with the other hosts over WireGuard) and TRON (Nile); one observer per host, quorum two thirds, the vault key held as CGGMP21 threshold shares created by the `TSS ceremony` workflow (development signer only in single-host `local` mode). Every signer checks each request against the chain's outbound rows before signing. Bitcoin checkpoints go through governance: host 0 proposes daily, every host votes on proposals whose header matches its Bitcoin view |
| assets | `KEEL` (6), `KUSD` (6), `BTC.BTC`, `ETH.ETH`, `ETH.USDT`, `TRON.TRX`, `TRON.USDT` |
| wallet | Keel Wallet 1.2.0, `window.keel` |

The node and the indexer send open CORS headers, so browser clients can call the testnet
directly.

## Funding an account

See `docs/deposits.md`: request a deposit address (wallet *Receive*, CLI or
SDK), send signet BTC or Nile USDT/TRX to it, and the chain credits the
vault asset after that chain's confirmations.

## Clients

A client is any service that signs actions for its users or holds a role.
Run the `Onboard client` workflow with the client's account address: it adds
the account to the attester set (and, if asked, makes it the param admin) and
sends it starting KEEL and KUSD. The first client is a marketplace sandbox that
signs up users with Keel Wallet and settles trades on the chain.

## Running a follower

Anyone can run a node that verifies the chain and serves the RPC without
voting. Fetch the genesis file (`/v1/sync/meta` names the snapshot height;
the genesis is published with each reset), then:

```
keel-node --me <seed-or-key>@<port> --genesis genesis.json --role follower \
  --bootstrappers <validator-pubkey>@<ip:port> --advertise <your-ip:port> \
  --sync-from https://testnet.keelchain.com/rpc \
  --storage-dir /var/lib/keel --rpc-listen 127.0.0.1
```

`--sync-from` installs the newest snapshot (checked against the full-state
hash and the tip hash the peer reports; add `--sync-verify <another rpc>` to
have a second node confirm the tip) and the epoch boundaries, then the node
fetches the blocks above it from its peers. A follower holds no block below
its snapshot, and `--retain-blocks N` lets it drop journal segments older than
N blocks below each new snapshot; `/v1/status` reports `oldest_block` and
`/v1/blocks` serves what it has. Validators keep the full history. Validators keep followers
connected through `--extra-peers` or, once the follower bonds, through the
chain state itself.

## Operations

Every host backs itself up daily (snapshot, config, and the indexer database
on host 0; encrypted, copied off the box), writes health metrics for
node_exporter every minute, and votes on pre-approved governance proposals
every ten minutes. Host 0 can run Prometheus, Alertmanager and Grafana. Binary
upgrades go through a `SoftwareUpgrade` proposal: a node stops at an
activation height it was not voted in for, so the matching release is
deployed first. `infra/testnet/README.md` has the details; `CHANGELOG.md`
announces resets and upgrades.

## Resetting

Run `Deploy testnet` with `reset_chain` checked. State, the indexer database
and the vault registrations are rebuilt from the seeds in the GitHub
environment. Balances and roles granted by onboarding are gone; run the
onboarding again for each client.

## Local

`infra/dev/devnet.sh` starts a four-validator devnet on one machine;
`infra/dev/e2e.sh` runs the trading and governance flows against it;
`infra/dev/custody-e2e.sh` exercises deposits and payouts with regtest
Bitcoin and a Tron mock. See `chain/README.md` for the flags.
