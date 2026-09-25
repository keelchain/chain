# keel-node

One validator of the Keelchain: authenticated p2p, Simplex consensus,
marshal (ordered finalized blocks + backfill), the VM, a mempool with
action gossip, and the HTTP/WS API (`crates/keel-rpc/API.md`).

## Run a devnet validator

```sh
keel-node --me 0@3000 --participants 0,1,2,3 --devnet --storage-dir /tmp/keel/0
keel-node --me 1@3001 --participants 0,1,2,3 --devnet --bootstrappers 0@127.0.0.1:3000 --storage-dir /tmp/keel/1
```

`--devnet` builds a genesis where every participant seed is a validator,
observer, arbitrator and attester and its account key
(`keel keygen --seed N`) is funded with KEEL, KUSD and BTC.BTC. The genesis
JSON is written to `<storage-dir>/genesis.json`; pass it to other nodes
with `--genesis` for a reproducible chain.

| Flag | Meaning | Default |
|---|---|---|
| `--me <seed>@<port>` | consensus key seed and p2p listen port | required |
| `--participants a,b,c` | seeds of all validators | required |
| `--bootstrappers <seed>@<ip:port>` | peers to dial first | none |
| `--storage-dir` | consensus archives, VM journal, snapshots, receipts | required |
| `--genesis <file>` / `--devnet` | genesis source | one required |
| `--rpc-port` | HTTP/WS API | p2p port + 2000 |
| `--snapshot-interval N` | VM snapshot every N blocks | 200 |
| `--blocks-per-epoch N` | consensus epoch length | params.epoch_length_blocks |
| `--log-level` | tracing level | info |

Ports: p2p `port`, Prometheus metrics `port+1000`, RPC `port+2000`.

## Durability

Every finalized block is appended to `<storage>/vm/journal.bin` before it is
acknowledged to consensus; receipts go to `receipts.jsonl`; snapshots to
`snapshot-<height>.bin`. On restart the newest snapshot is loaded and the
journal replayed above it; marshal then backfills any blocks missed while
down (`infra/dev/e2e.sh` exercises kill + restart).

## Scripts

- `infra/dev/devnet.sh [start|stop|kill i|restart i]` — 4-node devnet.
- `infra/dev/e2e.sh` — crossing orders across two nodes, balance and
  state-hash agreement on all nodes, restart catch-up.
- `sdk/ts` — TypeScript client; `keel` (`crates/keel-cli`) — CLI.
