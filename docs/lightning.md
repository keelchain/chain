# Bitcoin Lightning (decided 2026-09-10)

"Lightning is cheap for BTC and fast, need to support it … make any
parameters customizable from backoffice." This document describes the
design, the parameters, the operator setup, and what is verified.

## 1. Model: observer-run Lightning pools

Lightning payments need a hot node with channels. The chain cannot run one,
so each **observer** may run an LND node on behalf of the chain and register
it (`RegisterLightningNode { node_id }`). The chain keeps one **pool** per
observer: BTC the chain has lent that node (funded from the BTC vault by a
normal outbound owned by SYSTEM, `FundLightningPool`), the amount pending in
outgoing payments, and what it credited today. The sum of all pools is a
reserve account `lightning_pool` (debit-normal, under the BTC vault's
liabilities) so reserves-vs-liabilities still balance.

- **Deposit**: the marketplace asks the observer's invoice API for an invoice
  whose description is `keel:<owner address>` (`crates/keel-ln::deposit_description`).
  When it settles, the observer submits `ObserveLightningDeposit { invoice,
  preimage, amount_msat }`. The VM parses the BOLT11 invoice, checks the
  signature, the payee (must be that observer's node), the preimage against
  the payment hash, the amount, the caps, and credits the owner's
  `BTC.BTC` deposit balance; the pool balance grows by the same amount.
  Replay is impossible: one credit per payment hash.
- **Withdrawal**: a normal `Withdraw { asset: BTC.BTC, to: <bolt11>, amount }`.
  The vault hook sees a BOLT11 destination and hands it to
  `lightning::queue_payout`: the invoice must carry an amount equal to the
  withdrawal, be unexpired, within `lightning_max_withdraw_sats`; the user's
  escrow holds `amount + allowance`; the payout is **assigned** to the
  registered observer with the most free pool balance, with a deadline of
  `lightning_payout_timeout_blocks`. That observer pays with its LND node
  and reports `ObserveLightningPayout { outbound_id, preimage, fee_paid_msat,
  success }`. Success: the pool is debited `amount + fee_paid`, the unused
  allowance is refunded to the user, the outbound is `Confirmed` with the
  payment hash as its `tx_hash`. Failure or deadline: full refund
  (`LightningPayoutFailed`), the outbound is `Failed`.
- **Fee allowance** (what the chain lets the observer spend on routing):
  `max(amount × lightning_max_fee_bps / 10000, lightning_min_fee_sats)`.
  The marketplace charges exactly this as the "network fee" of a Lightning
  withdrawal; whatever is not spent comes back to the user on chain.
- **Sweep**: an observer whose node holds more than the chain thinks it
  should (or above `lightning_pool_cap_sats`) closes channels and sends the
  BTC to the vault's index-0 address, announcing the tx hash first
  (`AnnounceLightningSweep`); the vault's deposit hook recognises the
  announced hash, reduces the pool and books it back as reserves. An
  unannounced deposit to the vault's own address is refused.

Everything above is in `chain/crates/keel-vm/src/modules/lightning.rs`
(`crates/keel-vm/tests/lightning.rs` covers each rule) and the vault hooks in
`modules/vaults.rs`. The node serves `GET /v1/lightning` (params, pools,
open assignments with invoice/amount/owner, pending sweeps, `pool_total`).

## 2. Parameters (Backoffice → Chain → Lightning)

All are chain params, changed with `SetParam` by the param admin, i.e. from
the backoffice Chain page like every other parameter:

| Key | Default | Meaning |
|---|---|---|
| `lightning_enabled` | 1 | Off: invoices refused (deposits and payouts); on-chain BTC still works |
| `lightning_pool_cap_sats` | 50,000,000 (0.5 BTC) | Most BTC one observer's node may hold for the chain; funding above it is refused |
| `lightning_max_deposit_sats` | 10,000,000 | Largest single Lightning deposit |
| `lightning_max_withdraw_sats` | 10,000,000 | Largest single Lightning payout |
| `lightning_max_fee_bps` | 50 (0.5%) | Routing fee allowance in bps of the amount |
| `lightning_min_fee_sats` | 10 | Allowance floor for tiny payouts |
| `lightning_payout_timeout_blocks` | 600 | Blocks an observer has to pay before the chain refunds |
| `lightning_daily_cap_sats` | 0 (none) | Total deposits one observer may credit per day |

The marketplace's own fee schedule (flat platform fee per withdrawal, Fees
page) applies on top exactly as for on-chain withdrawals. Note:
that flat fee is USD-pegged and dominates small Lightning withdrawals; if the
goal is "cheap", give Lightning its own tier on the Fees page.

## 3. Operator setup (observer)

1. Run `lnd` (0.19.x) with a funded wallet and channels to well-connected
   peers. Keep `admin.macaroon` readable by the observer.
2. Add to the observer TOML:

   ```toml
   [lightning]
   rest_url = "https://127.0.0.1:8080"
   macaroon_path = "/path/to/admin.macaroon"   # or macaroon_hex
   tls_cert_path = "/path/to/tls.cert"         # or tls_insecure = true on regtest
   api_listen = "127.0.0.1:7201"               # invoice API for the marketplace
   invoice_expiry_secs = 3600
   poll_secs = 2
   ```

   On start the observer registers its node id on chain once, then loops:
   settled invoices bound to an owner → `ObserveLightningDeposit`; payouts
   assigned to it → pays and reports. `GET /v1/lightning/info` on the API
   answers node id, observer address, channel balance, invoice expiry.
3. Fund the pool from the vault (needs the observer key):
   `keel send --secret <observer secret> lightning fund <sats> <node on-chain address>`.
   The vault pays a normal batched outbound to the node's wallet; when the
   observer sees that deposit it credits the pool (`LightningPoolFunded`).
4. Sweep excess: `keel send … lightning sweep <tx_hash> <sats>` **before**
   broadcasting the on-chain transaction to the vault's index-0 address.

`infra/dev/lightning-e2e.sh` stands up two regtest LND nodes with a
channel, registers observer 0, funds its pool with 0.03 BTC, deposits
25,000 sats by invoice and withdraws 12,000 sats to an external invoice.

## 4. Marketplace and apps

- `apps/marketplace/src/modules/chain/lightning.ts`: a small BOLT11 decoder
  (bech32 checksum, network, amount, payment hash, expiry, description) and
  `ChainLightning` (node status, invoice from the least-loaded registered
  observer in `CHAIN_LIGHTNING_OBSERVERS`, withdrawal pricing, admin view).
- Routes: `GET /v1/wallet/lightning` (availability, limits, fee schedule,
  liquidity, invoice expiry), `POST /v1/wallet/lightning/invoice {amount}`
  (sats), `POST /v1/wallet/lightning/decode {invoice}`,
  `GET /v1/wallet/withdrawals/quote?…&invoice=` (lightning pricing),
  `POST /v1/wallet/withdrawals` with a BOLT11 `toAddress` (amount must equal
  the invoice, `speed` forced to express, allowance as the network fee),
  `GET /v1/admin/chain/lightning` (super admin: pools, assignments, sweeps,
  observer reachability). Invoices are never offered as "recent addresses".
- Wallet history: the indexer materialises `LightningDepositCredited` and
  `LightningPayoutSettled` as transfers of kind `lightning_deposit` /
  `lightning_payout`; the marketplace shows them as deposit/sendout rows on
  network `lightning` with the chain tx id. Deposit notifications fire from
  the deposit watcher as for on-chain credits.
- Web: Wallet → Deposit → BTC on Bitcoin gets an "On-chain | ⚡ Lightning"
  switch: amount, "Create invoice", QR + copy + countdown. Wallet → Send:
  paste an invoice as the destination; the amount locks to the invoice, the
  speed picker disappears, the fee shows the allowance.
- Backoffice: Chain page → Lightning group (all params, with help) and a
  Lightning card (pools, open payouts, sweeps, observer nodes).
- Explorer: Vaults page → "Lightning (BTC)" card (indexer `GET /v1/lightning`).

## 5. Verified (2026-09-10, local testnet)

`lightning-e2e.sh` PASS; through the marketplace API as alice: a 30,000 sat
invoice paid from the second LND node was credited in about 8 s and shows
as a Lightning deposit with its chain tx id; a 15,000 sat withdrawal to an
LND invoice was paid by observer 0 and marked settled 4 s after the request.
Test counts: Rust 178 (indexer 20), marketplace 2368, web 699, backoffice
158, explorer 16.

## 6. Limits and next steps

- One LND per observer, keys on the observer host: the pool is the
  observer's hot exposure, capped by `lightning_pool_cap_sats`. Observer
  bonds must exceed the cap on mainnet.
- Payout assignment is by free balance only; no channel-liquidity
  awareness. An observer that cannot route reports failure and the chain
  refunds; a stuck observer is refunded at the deadline.
- No LNURL / Lightning Address yet; invoices are pasted or scanned.
- Mainnet: the deposit invoice API of each observer must be reachable by
  the marketplace over TLS with an allow-list; today it is plain HTTP on
  localhost.
