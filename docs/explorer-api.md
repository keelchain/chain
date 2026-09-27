# Keel explorer — indexer API contract (v1)

The explorer web app (`apps/explorer`) talks only to the indexer
(`chain/crates/keel-indexer`), never to a validator directly. One indexer
process serves one network; the explorer is configured with one base URL per
network and shows a switcher:

```
VITE_NETWORKS='[{"id":"testnet","name":"Testnet","api":"https://testnet.keelchain.com/api"},{"id":"mainnet","name":"Mainnet","api":"https://api.keelchain.com"}]'
```

The indexer keeps full history in Postgres. It backfills from the node's
`GET /v1/blocks/{h}` and `GET /v1/blocks/{h}/receipts` (served from the
node's journals for any height) and then follows `/v1/ws`. It re-verifies
the chain of `state_hash`es it receives against `/v1/status` and flags a
mismatch on `/v1/health`.

The indexer listens on `127.0.0.1:6100` by default (`--listen`); a mainnet
instance conventionally uses `6101`. Do not use 6000–6063 or 6665–6669:
browsers refuse `fetch` to those ports (WHATWG "bad ports"), so the explorer
would silently fail.

All amounts are decimal strings in smallest units plus a `decimals` where
an asset is involved. Addresses and hashes are hex. Paging: `?limit=&cursor=`,
responses carry `next_cursor` when more exist. Every list is newest first.

| Method | Path | Returns |
|---|---|---|
| GET | `/v1/health` | `{network, chain_id, indexed_height, node_height, lag, state_hash_ok, started_at}` |
| GET | `/v1/stats` | `{height, block_time_ms_avg, tps_1h, actions_24h, accounts, validators, tvl_usd, assets:[{asset, supply, holders}], fees_24h:[{asset, amount}]}` |
| GET | `/v1/blocks?limit=&cursor=` | `{blocks:[Block], next_cursor}` |
| GET | `/v1/blocks/{height}` | `Block & {receipts:[Tx], events:[Event]}` |
| GET | `/v1/txs?limit=&cursor=&signer=&module=&ok=` | `{txs:[Tx], next_cursor}` |
| GET | `/v1/txs/{tx_id}` | `Tx & {action (decoded JSON), block: Block}` |
| GET | `/v1/accounts/{addr}` | `{address, nonce, tier, budget, balances:[{asset, account_type, balance, decimals}], first_seen_height, tx_count, deposit_addresses:[{chain, index, address}], validator?:{...}, offers_count, trades_count}` |
| GET | `/v1/accounts/{addr}/txs` | like `/v1/txs` filtered by signer or counterparty |
| GET | `/v1/accounts/{addr}/orders`, `/offers`, `/trades` | `{orders|offers|trades:[...], next_cursor}` newest first |
| GET | `/v1/accounts/{addr}/transfers` | `{transfers:[{tx_id, height, timestamp, asset, amount, from, to, kind}]}` (Transferred, DepositCredited, OutboundConfirmed, OrderFilled legs, TradeReleased) |
| GET | `/v1/assets` | `[{asset, decimals, kind, supply, holders, reserves?, chain?}]` |
| GET | `/v1/assets/{asset}` | asset + `{holders_top:[{address, balance}], transfers_24h}` |
| GET | `/v1/markets` | `[{pair, base, quote, last_price, volume_24h_base, volume_24h_quote, trades_24h, best_bid, best_ask}]` |
| GET | `/v1/markets/{pair}` | market + `{book:{bids:[[price,size]], asks:[[price,size]]}, house_quote}` (book fetched live from the node) |
| GET | `/v1/markets/{pair}/fills?limit=&cursor=` | `{fills:[{tx_id, height, timestamp, price, quantity, quote, taker, maker_order_id, fee}]}` |
| GET | `/v1/markets/{pair}/candles?interval=1m|5m|1h|1d&from=&to=` | `[{t, o, h, l, c, v}]` |
| GET | `/v1/orders/{id}` | order record + fills |
| GET | `/v1/offers?asset=&side=&status=&owner=&limit=&cursor=` | `{offers:[Offer], next_cursor}` (public fields only; `status` is `active|paused|closed`) |
| GET | `/v1/offers/{id}` | Offer + trades |
| GET | `/v1/trades/{id}` | Trade (status history, dispute if any) |
| GET | `/v1/validators` | `[{address, consensus_key, self_bond, delegated, power, jailed, blocks_proposed_24h?, uptime?}]` |
| GET | `/v1/epochs` | `[{epoch, start_height, validators}]` |
| GET | `/v1/vaults` | `[{chain, epoch, address_count, reserves:[{asset, amount}], liabilities, fee_rate, halted}]` |
| GET | `/v1/vaults/{chain}/deposits?status=&owner=&limit=&cursor=` | `{deposits:[...]}` (pending, credited, held, rejected; with the external tx hash and depth) |
| GET | `/v1/vaults/outbounds?status=&owner=&limit=&cursor=` | `{outbounds:[...], batches:[...]}` |
| GET | `/v1/governance/proposals` | `[Proposal]` with tallies and status |
| GET | `/v1/governance/proposals/{id}` | Proposal + votes |
| GET | `/v1/governance/params` | `{params:{key: value}, history:[{height, key, from, to, tx_id}], live}`; nested node params are flattened with dots (`budget.base`) and values are strings |
| GET | `/v1/search?q=` | `{kind: block|tx|account|asset|market|offer|trade|validator, ref}` best match plus `suggestions` |
| WS | `/v1/ws` | `{type:"block", block: Block & {tx_count}}` per block; `{type:"tx", tx: Tx}` optional |

Types:

```
Block: {height, timestamp, state_hash, tx_count, ok_count, event_count, proposer?}
Tx:    {tx_id, height, index, timestamp, signer, module, kind, ok, error?:{code, message}, events:[Event]}
Event: {type, ...fields}  (serde of keel_vm::Event, addresses hex)
Offer: {id, owner, side, asset, fiat_currency, payment_method, margin_bps, min_amount, max_amount, payment_window_secs, country, min_tier, status, created_height}
Trade: {id, offer_id, buyer, seller, asset, amount, fee, fiat_amount, fiat_currency, status, started_height, deadline, paid_at?, closed_height?, dispute?}
```

Errors: `{error:{code, message}}` with 400/404/500.

Live fallbacks: the order book, best bid/ask, balances, vault state and
governance records are read from the node on request (and the DB copy
refreshed) so objects older than the indexer's first block still resolve;
when the node is unreachable the last snapshot is served and `/v1/health`
reports `node_reachable: false`.

## WebSocket

`/v1/ws` uses the same envelope as the node's socket (`chain/crates/keel-rpc/API-WS.md`):
channels `blocks`, `txs` and `account:<hex>`, a per-connection `seq`, a `gap`
frame on lag and a heartbeat. A socket that never subscribes receives
`blocks` and `txs`.
