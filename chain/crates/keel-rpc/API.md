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
| GET | `/v1/blocks/{height}/receipts` | `{height, receipts[]}` (last 10,000 blocks) |
| WS | `/v1/ws` | one JSON message per applied block: `{height, timestamp, state_hash, receipts[], events[], books{pair:{best_bid,best_ask,last_price}}}` |

Sub-states owned by other modules (offers, trades, vaults, proposals,
staking) are served as their serde JSON; field names follow the Rust
structs in `crates/keel-vm/src/modules/`.

Error codes on `/v1/actions`: `BAD_SIGNATURE`, `WRONG_CHAIN`, `BAD_NONCE`,
`BUDGET_EXHAUSTED`, `BLOCK_CAP`, `PAUSED`, `UNAUTHORIZED`, `INVALID`. A
nonce may run up to 64 ahead of the chain nonce (pipelining); actions are
selected per signer in nonce order.

## Required by marketplace (not yet served)

The marketplace chain client (`apps/marketplace/src/modules/chain/`) reads
these in addition to the table above. Until the node serves them the client
degrades as noted; nothing else in the marketplace depends on them.

| Method | Path | Used for | Without it |
|---|---|---|---|
| GET | `/v1/vaults/{chain}/addresses/{index}` → `{chain, index, owner, address}` | the deposit ADDRESS STRING of an assigned index (`keel_chains::deposit_address` over the active vault's public key + chain code; the TS side has no secp256k1) | `issueAddress` fails with `ADDRESS_DERIVATION_UNAVAILABLE` after the index is assigned; a retry resolves once the route exists. The client also accepts an `address` field on the rows of `/v1/vaults/{chain}/addresses`. |
| GET | `/v1/vaults/{chain}/addresses/lookup?address=` → same row | reverse lookup of a deposit address | only addresses issued through this marketplace resolve |
| GET | `/v1/vaults/deposits?status=&owner=` → `{deposits:[{chain, asset, tx_hash, index, deposit_index, owner, address?, amount, confirmations, required_confirmations, status, first_seen_at?, settled_at?, rejection_reason?}]}` | pending / rejected deposit notices and the per-customer deposit history | those lists are empty; credited deposits still arrive over `/v1/ws` (`DepositCredited`) |
