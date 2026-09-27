# Keel node HTTP/WS API (v1)

Base: `http://127.0.0.1:<rpc-port>` (devnet: 5000+i). JSON everywhere.
Amounts are integers in smallest units and may exceed 2^53: parse as
bigint. Addresses are 64-hex ed25519 public keys in paths; inside a signed
action's JSON they are 32-element byte arrays (serde form).

| Method | Path | Returns |
|---|---|---|
| POST | `/v1/actions` | body: `SignedAction` JSON (`{envelope:{signer,nonce,chain_id,action},signature}`) → 202 `{tx_id, admitted:true}` or 422 `{admitted:false, code, error}` |
| GET | `/v1/status` | `{chain_id, height, timestamp, state_hash, validators[], mempool, accounts, orders}` |
| GET | `/v1/params` | governance parameters |
| GET | `/v1/accounts/{addr}` | `{nonce, budget{used,limit,cancel_limit,remaining,earned}, tier, tier_expires_at, balances[{asset,account_type,balance}]}` |
| GET | `/v1/accounts/{addr}/orders` | `{open:[OrderRecord]}` |
| GET | `/v1/accounts/{addr}/trades` | `{trades:[Trade]}` |
| GET | `/v1/markets` | `{markets:[summary]}` |
| GET | `/v1/markets/{pair}` | summary: cfg, house quote, last price, best bid/ask, volumes |
| GET | `/v1/markets/{pair}/book?depth=20` | `{bids:[{price,size,orders}], asks:[…], house, height}` |
| GET | `/v1/orders/{id}` | OrderRecord |
| GET | `/v1/offers?asset=&side=&fiat=` | `{offers:[Offer]}` (open offers only) |
| GET | `/v1/offers/{id}` | Offer |
| GET | `/v1/trades/{id}` | Trade (+ `dispute` when one exists) |
| GET | `/v1/vaults/{chain}` | `{chain, vault, state}` (vault registrations for the chain; whole vaults sub-state) |
| GET | `/v1/vaults/{chain}/addresses?from=0&limit=100` | deposit index → owner rows |
| GET | `/v1/vaults/outbounds?status=` | `{outbounds:[Outbound]}` |
| GET | `/v1/gov/proposals` | `{proposals:[Proposal], house_operator}` |
| GET | `/v1/gov/proposals/{id}` | Proposal |
| GET | `/v1/staking/validators` | `{consensus:[hex keys], staked:[{consensus_key,power}], staking}` |
| GET | `/v1/receipts/{tx_id}` | `{index, tx_id, signer, ok, error{code,message}?, events[]}` (404 until applied) |
| GET | `/v1/clients/{addr}` → `{address, attester, fee, caps, earned, usage_paid, attested_accounts, usage_prices}` | a client's retail schedule and what it earned and paid |
| GET | `/v1/treasury` → `{height, balances:{treasury,validator_rewards,burn}, last_buyback, params, fee_split_bps}` | system balances per asset and the epoch buyback record |
| GET | `/v1/ready/{chain}?max_checkpoint_age=` → `{chain, ready, reasons, height, vault, checkpoint, last_deposit_credited, outbound_pending, halted, fee_rate}` | whether a client may turn that network on; what a per-network switch polls |
| GET | `/v1/sync/meta` → `{height, state_hash, last_hash, schema, boundaries:[{epoch,height,digest}]}` | newest snapshot for state sync (`keel-node --sync-from`) |
| GET | `/v1/sync/snapshot` → bytes | the snapshot itself (`KEEL` header, schema, borsh state) |
| GET | `/v1/blocks/{height}/receipts` | `{height, receipts[]}` (last 10,000 blocks) |
| WS | `/v1/ws` | one JSON message per applied block: `{height, timestamp, state_hash, receipts[], events[], books{pair:{best_bid,best_ask,last_price}}}` |

Sub-states owned by other modules (offers, trades, vaults, proposals,
staking) are served as their serde JSON; field names follow the Rust
structs in `crates/keel-vm/src/modules/`.

Error codes on `/v1/actions`: `BAD_SIGNATURE`, `WRONG_CHAIN`, `BAD_NONCE`,
`BUDGET_EXHAUSTED`, `BLOCK_CAP`, `PAUSED`, `UNAUTHORIZED`, `INVALID`. A
nonce may run up to 64 ahead of the chain nonce (pipelining); actions are
selected per signer in nonce order.

## Custody (client-owned vaults)

| Route | What |
|---|---|
| `GET /v1/custody` | every client-owned vault: `custodian`, `chain`, `vault` (key, chain code, epoch), `signer_url`, `address` (index 0), `next_deposit_index`, `deposit_owners`, `reserves[]` (`asset`, `reserve`, `liabilities`, `halted`) |
| `GET /v1/custody/{addr}` | one client's vaults and reserves, plus the number of accounts in its custody |
| `GET /v1/custody/{addr}/{chain}/addresses?from&limit&owner` | deposit addresses of that vault with their owners |

Outbound rows and batches under `/v1/vaults/outbounds` carry `custodian`
(null for the network vault).

## WebSocket

`/v1/ws` carries filtered, sequenced subscriptions (`blocks`, `account:`,
`pair:`, `deposits:`, `book:`) with a `gap` frame when a client falls behind
and replay from a height; the protocol is in `API-WS.md`. A socket that never
subscribes receives `blocks`, which is the earlier per-block feed.

## Stability

`/v1` is frozen: routes and fields are added, never removed or renamed. A
deprecation is announced thirty days ahead in the changelog and keeps working
meanwhile. Action submission may require an API key (`Authorization: Bearer`)
on the public testnet; reads never do.
