# The public testnet

Live since 2026-09-24 at keelchain.com, deployed only by GitHub Actions
(`infra/testnet/README.md`).

| Piece | Where |
|---|---|
| site | https://keelchain.com (`site/`, wallet pages, wallet packages under `/wallet/downloads/`) |
| explorer | https://testnet.keelchain.com (`apps/explorer`, `VITE_BASE=/`) |
| Harbor | [harbor.keelchain.com](https://harbor.keelchain.com/) (`apps/harbor`): the clients' account, where a plan is bought and managed. Static; it reads the API below and every change is an action signed in the wallet |
| read API | https://testnet.keelchain.com/api/v1/… (`keel-indexer`, `docs/explorer-api.md`) |
| node RPC | https://testnet.keelchain.com/rpc/v1/… (`keel-node`; status, accounts, params, `gov/roles`, `gov/proposals`, receipts, WebSocket at `/rpc/v1/ws`) |
| chain | id 3, `_KEEL_CHAIN` namespace, external network signet; the validator set is the host set in `DEPLOY_HOSTS` (one host today; consensus is BFT, so four validators tolerate one fault and three tolerate none), 600-block epochs; genesis built by the deploy workflow from every host's public identity plus `infra/testnet/genesis-params.json` (voting 60 blocks, timelock 10, proposal deposit 1000 KEEL) |
| vaults | BTC (signet, pruned bitcoind on host 0, shared with the other hosts over WireGuard) TRON (Nile), BSC (the BNB Smart Chain testnet, chain id 97, the same address as Ethereum) and SOL (Solana devnet, an Ed25519 vault key signed by FROST); one observer per host, quorum two thirds, the vault key held as CGGMP21 threshold shares created by the `TSS ceremony` workflow (development signer only in single-host `local` mode). Every signer checks each request against the chain's outbound rows before signing. Bitcoin checkpoints go through governance: host 0 proposes daily, every host votes on proposals whose header matches its Bitcoin view |
| assets | `KEEL` (6), `USDT` (6), `BTC.BTC`, `TRON.TRX`, `TRON.USDT`, `BSC.BNB`, `BSC.USDT`, `SOL.SOL`, `SOL.USDC`; `ETH.ETH` and `ETH.USDT` once the Ethereum vault is registered. Per network, with the live state: `networks.md` |
| wallet | Keel Wallet 1.2.0, `window.keel` |

The node and the indexer send open CORS headers, so browser clients can call the testnet
directly.

## Funding an account

See `docs/deposits.md`: request a deposit address (wallet *Receive*, CLI or
SDK), send signet BTC, Nile USDT/TRX, BSC-testnet USDT/BNB or devnet SOL/USDC to it, and the chain credits the
vault asset after that chain's confirmations.

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

Your users' testnet deposits land in **your vault**, so register one per
chain first (`RegisterCustodyVault`, a `keel-tss serve` or a signing hook
of your own reachable by the observers) and hand out addresses with
`RequestCustodyAddress`. The faucet funds accounts, not vaults: top up
your vault's index-0 address from a signet or Nile faucet for gas and
float. Your operator account's starting KEEL and USDT come from onboarding
(below), not the faucet.

</div>
<div class="tab" data-mode="noncustodial">

Your users' deposits land in the **network vault** on each chain, with
addresses the chain assigns through `RequestDepositAddress`. While your
plan runs they need no KEEL; send each user the faucet's USDT or your own
if a test trade needs a balance. Your operator account's starting KEEL and
USDT come from onboarding (below), not the faucet.

</div>
<div class="tab" data-mode="user">

Claim 100 KEEL and 100 USDT from `faucet.keelchain.com`, lock 1 KEEL from
the wallet's *Action capacity* card, then request an address and send
test coins to it. Coins can also stay in your own wallet, under a key from
your seed, and move into Keel later (`docs/wallet.md` §5b).

</div>
</div>

## Clients

A client is any service that signs actions for its users or holds a role.
Run the `Onboard client` workflow with the client's account address: it adds
the account to the attester set and sends it starting KEEL and USDT. The
client then connects that account to Keel Harbor, locks its tier's KEEL
and buys its plan there (`docs/clients.md`, Pricing).

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

After onboarding, register your custody vault per chain from Harbor
(Business) or with `RegisterCustodyVault`, attest your users, and request
their addresses in your vault. Your signer must be reachable by the
testnet observers to sign payouts. `docs/models.md`, Model A, is the
procedure; `infra/dev/custody-e2e.sh` exercises the same deposits and
payouts locally against regtest Bitcoin and a Tron mock.

</div>
<div class="tab" data-mode="noncustodial">

After onboarding, attest your users and connect your site to Keel Wallet
(`docs/connect-keel-wallet.md`). The first client is a marketplace
sandbox that signs up users with Keel Wallet and settles trades on the
chain; `apps/example-client` is the shortest complete integration.

</div>
<div class="tab" data-mode="user">

Nothing to onboard. A wallet user is any address: claim from the faucet,
lock 1 KEEL and act. You become a client's user only when an exchange
attests your account.

</div>
</div>

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

## Faucet

`faucet.keelchain.com` sends 100 KEEL and 100 USDT to any account address,
once per address a day and a few times per network address. It is an
ordinary account that each deploy tops up from the genesis account; its
sends are plain transfers on the explorer. Scripted: `POST https://testnet.keelchain.com/api/v1/faucet` with
`{"address": "<64 hex>"}`; `GET /v1/faucet` for the amounts and what it
holds, `GET /v1/faucet/claims` for history. Clients' operator accounts are
funded through onboarding, not the faucet.

The faucet is also the first thing a new address needs. There is no free
action budget: an address acts on the capacity of KEEL it has locked, so
claim KEEL, then lock 1 KEEL from the wallet's *Action capacity* card
(a hundred actions a day, returned in full after unlocking). Before that
the chain refuses every action, including the request for a deposit
address, with `BUDGET_EXHAUSTED`, or `NOT_ACTIVATED` while the address
holds less than `budget.activation_keel` (0.1 KEEL)
(`docs/governance.md`).

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
