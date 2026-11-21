# Keel Harbor

The account portal for clients of the Keelchain (exchanges and other
attesters): buy the membership plan, lock KEEL, watch usage, set retail
fees and run the custody vault tools. Served at
`https://harbor.keelchain.com`.

Harbor has no backend and holds no key. Every number is read from the
chain's public API, and every change is one chain action signed in the
Keel Wallet browser extension and submitted to the node RPC. The CLI and
the SDK do exactly the same things; Harbor is the client endpoint with
buttons.

React 19 + Vite, the same toolchain and visual language as
`apps/explorer`, with a light and a dark theme (it follows the system).

```sh
npm ci
npm run dev            # http://127.0.0.1:5178 (VITE_NETWORKS from .env / .env.example)
npm test               # pure billing logic + page tests against the in-browser mock
npm run build          # dist/ static site (tsc + vite build)
VITE_API_MOCK=1 npm run dev   # demo client, no node and no wallet; ?demo=none|active|suspended|owed|expired|visitor
```

`@keelchain/sdk` is the local package in `sdk/ts`; build it first
(`npm ci && npm run build` there) when its `dist/` is missing or stale.

## Configuration (build time, Vite env)

| Var | Meaning |
|---|---|
| `VITE_NETWORKS` | JSON list of `{id, name, api, rpc?}`; the first entry is the default. `api` is the indexer, `rpc` the node RPC; without `rpc` it is `/rpc` next to the indexer's `/api`. Ids must be `[a-z0-9-]+`. |
| `VITE_EXPLORER_URL` | Base of the block explorer that transaction and account links open; the network id is appended (`<url>/testnet/tx/<id>`). Default `https://testnet.keelchain.com`. |
| `VITE_BASE` | Base path when hosted under a prefix. |
| `VITE_API_MOCK` | `1` serves a demo client from the in-browser mock, already connected; nothing is signed. |

URLs carry the network as the first path segment (`/testnet/plans`).
Serve `dist/` from any static host with SPA fallback to `index.html`.

## What it reads and writes

The browser calls these routes directly, so both services must answer CORS
for Harbor's origin.

Node RPC (`rpc`):

| Route | For |
|---|---|
| `GET /v1/clients/{addr}` | The client record: plan, lock, tier table, prices, owed, fees, earned. Also called with the all-zero address to show the tiers before a wallet is connected. |
| `GET /v1/accounts/{addr}` | Balances, the lock and pending unlocks; the nonce before each signature. |
| `GET /v1/custody/{addr}` | Custody vaults, pending rotations and floats (Business). |
| `GET /v1/params` | `budget.unlock_delay_secs`. |
| `POST /v1/actions` | Submitting the action the wallet signed. |
| `GET /v1/receipts/{tx_id}` | Polled until the receipt of that action exists. |

Indexer (`api`):

| Route | For |
|---|---|
| `GET /v1/accounts/{addr}/txs?limit=50&cursor=…` | Billing history: the plan and usage events in transactions that name the client. |
| `GET /v1/assets` | Decimals per asset. |

Actions it sends: `BuyPlan`, `SetPlanRenewal`, `LockBudget`,
`UnlockBudget`, `SetClientFee`, `PlaceOrder` (a market buy of KEEL),
`RotateCustodyVault`, `CancelCustodyRotation`, `SwapCustody`.

## Pages

- **Landing** (no wallet): what Harbor is and the live tier cards. An
  account that is not an attester is told Harbor is for clients and how one
  becomes a client.
- **Overview**: plan status, days left, renewal, the allowance, action pool
  and lock meters, what is owed, and the single next thing to do.
- **Plans**: each tier against the client's lock and balance, buying and
  upgrading with the charge shown before signing, and the renewal setting.
- **Funds**: balances, how the billing asset reaches the account, buying
  KEEL, locking and unlocking with a warning when an unlock would suspend
  the plan.
- **Usage**: the usage prices, the allowance this period, totals paid and
  the billing history.
- **Business**: retail fees and what they earned, attested accounts, and
  the custody vault, rotation and float swap tools.

## Where the rules live

`src/lib/plan.ts` mirrors the chain's billing rules
(`chain/crates/keel-vm/src/modules/clients.rs`) as pure functions: plan
status, what each tier still needs, the upgrade credit and the next step.
The chain is the authority; the page only says in advance what a signature
will do, and shows the receipt afterwards.
