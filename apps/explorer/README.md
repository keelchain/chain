# KEEL block explorer

React 19 + Vite. Reads only the indexer API (`chain/crates/keel-indexer`,
contract in `docs/explorer-api.md`); one indexer per network, chosen
at runtime with the switcher in the header. URLs carry the network as the
first path segment (`/testnet/blocks/42`, `/mainnet/tx/<id>`).

```sh
npm ci
npm run dev            # http://127.0.0.1:5177 (VITE_NETWORKS from .env / .env.example)
npm test               # unit + page tests against the in-browser mock
npm run build          # dist/ static site (tsc + vite build)
EXPLORER_LIVE_API=http://127.0.0.1:6100 npx vitest run src/App.live.test.tsx   # every page against a live indexer
```

Configuration (build time, Vite env):

| Var | Meaning |
|---|---|
| `VITE_NETWORKS` | JSON list of `{id, name, api}`; first entry is the default. Ids must be `[a-z0-9-]+`. |
| `VITE_API_MOCK` | `1` serves every network from the deterministic in-browser mock (design work, demos, tests). |
| `VITE_BASE` | Base path when hosted under a prefix. |

Deploying for testnet + mainnet: run two indexers (their `--network` and
databases differ), build once with
`VITE_NETWORKS='[{"id":"testnet","name":"Testnet","api":"https://testnet-api.example"},{"id":"mainnet","name":"Mainnet","api":"https://api.example"}]'`
and serve `dist/` from any static host with SPA fallback to `index.html`.
The indexer answers CORS for any origin and exposes `/v1/ws` for the live
tip. Never put an indexer on ports 6000–6063 or 6665–6669: browsers refuse
`fetch` to them.

Pages: home (stats, latest blocks/txs, live tip), blocks, txs (decoded
actions + events), accounts (balances, budget, transfers, orders, offers,
trades, deposit addresses), assets, markets (live book, depth chart,
candles, fills), orders, P2P offers and trades (status history, disputes),
validators + epochs, vaults (deposits/outbounds with external chain links),
governance (proposals, votes, params + history), search.
