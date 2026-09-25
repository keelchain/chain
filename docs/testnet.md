# The public testnet

Live since 2026-09-24 at keelchain.com, deployed only by GitHub Actions
(`infra/testnet/README.md`).

| Piece | Where |
|---|---|
| site | https://keelchain.com (`site/`, wallet pages, wallet packages under `/wallet/downloads/`) |
| explorer | https://testnet.keelchain.com (`apps/explorer`, `VITE_BASE=/`) |
| read API | https://testnet.keelchain.com/api/v1/… (`keel-indexer`, `docs/explorer-api.md`) |
| node RPC | https://testnet.keelchain.com/rpc/v1/… (`keel-node`; status, accounts, params, `gov/roles`, `gov/proposals`, receipts, WebSocket at `/rpc/v1/ws`) |
| chain | id 3, one validator, `_KEEL_CHAIN` namespace, external network signet; genesis built by the deploy from the validator seed plus `infra/testnet/genesis-params.json` (voting 60 blocks, timelock 10, proposal deposit 1000 KEEL) |
| vaults | BTC (signet, pruned bitcoind on the box) and TRON (Nile); one observer, quorum 1, development signer. Bitcoin checkpoints are proposed daily by `keel-btc-checkpoint.timer` |
| assets | `KEEL` (6), `KUSD` (6), `BTC.BTC`, `ETH.ETH`, `ETH.USDT`, `TRON.TRX`, `TRON.USDT` |
| wallet | Keel Wallet 1.1.0, `window.keel` (alias `window.stt` for earlier clients) |

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
