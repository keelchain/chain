# WebSocket subscriptions (v1)

Both `/rpc/v1/ws` (the node) and `/api/v1/ws` (the indexer) speak the same
envelope. One socket carries any number of channels; every server frame has a
per-connection `seq` (monotonic from 1) and the block `height` it belongs to,
so a client can tell exactly what it received and what it missed.

## Client → server

```json
{"op":"subscribe","channels":["blocks","account:<hex>","pair:BTC-KUSD","deposits:BTC:<hex>","book:BTC-KUSD?depth=20"],"from_height":120}
{"op":"unsubscribe","channels":["pair:BTC-KUSD"]}
{"op":"ping"}
```

`subscribe` replaces the channel set. `from_height` replays the event channels
(`account:`, `pair:`, `deposits:`) from that height up to the tip before live
delivery starts (at most the last 10,000 blocks on the node). A connection
that never subscribes receives `blocks` (node) or `blocks` and `txs`
(indexer), which is what the earlier firehose sent.

## Channels

| channel | where | what arrives |
|---|---|---|
| `blocks` | node, indexer | `block`: the node sends the whole block (`timestamp`, `state_hash`, `receipts`, `events`, `books` top of book per pair); the indexer sends its block summary |
| `txs` | indexer | `tx`: one transaction summary per transaction |
| `account:<hex>` | node, indexer | node: `event` with `data.kind = receipt` for every receipt signed by or touching the address (transfers to it, trades it is in, deposits credited to it, ...) and `data.kind = event` for end-of-block events about it; indexer: `tx` summaries signed by it |
| `pair:<PAIR>` | node | `event` per order event on that pair (`OrderAccepted`, `OrderFilled`, `OrderCancelled`, `OrderRejected`, with `tx_id`) and `book_top` after each block (`best_bid`, `best_ask`, `last_price`) |
| `deposits:<CHAIN>:<hex>` | node | `event` per deposit-side event for that owner on that chain: `DepositAddressAssigned`, `DepositCredited`, `WithdrawalQueued` |
| `book:<PAIR>?depth=N` | node | `book_snapshot` on subscribe (`bids`, `asks` as `[price, size]`, best first, at most N levels a side), then `book_delta` per block with only the changed levels: `changes` of `[side, price, size]`, size `"0"` = level gone |

## Server → client

| type | fields | meaning |
|---|---|---|
| `subscribed` / `unsubscribed` | `channels` | the channel set now in force |
| `block` | see above | one finalized (node) or indexed (indexer) block |
| `event` | `channel`, `data` | one receipt or event on an event channel |
| `book_top`, `book_snapshot`, `book_delta` | `pair`, ... | order-book streams |
| `gap` | `missed`, `from_height`, `to_height`, `resync` | this connection fell behind and frames were dropped; nothing between `from_height` and `to_height` was delivered. `resync` names the REST routes to read them from; book channels get a fresh `book_snapshot` right after |
| `heartbeat` | | every 15 s, with the current `height` |
| `pong` | | answer to `ping` |
| `error` | `message` | a bad op or channel; the subscription set is unchanged |

A client that sees `seq` jump has missed frames too (a proxy or its own
buffer dropped them) and should treat it as a `gap`. The SDK's `KeelSocket`
does both: it raises a synthetic `gap` on a sequence hole, and on a dropped
connection it reconnects with backoff and re-subscribes with `from_height`
set to the last height it saw.

## Example

```ts
import { KeelSocket } from "@keelchain/sdk";

const ws = new KeelSocket("wss://testnet.keelchain.com/rpc/v1/ws");
ws.on("event", (f) => console.log(f.channel, f.data));
ws.on("book_delta", (f) => applyDelta(f.pair, f.changes));
ws.on("gap", async (f) => {
  for (let h = f.from_height; h <= f.to_height; h++) await reconcile(h); // GET /v1/blocks/{h}/receipts
});
await ws.subscribe([`account:${me}`, "book:BTC-KUSD?depth=10"]);
```
