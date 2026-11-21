# Keel node HTTP/WS API (v1)

Base: `http://127.0.0.1:<rpc-port>` (devnet: 5000+i). JSON everywhere.
Amounts are integers in smallest units (for KEEL: orcas, 1,000,000 to the
KEEL) and may exceed 2^53: parse as
bigint. Addresses are 64-hex ed25519 public keys in paths; inside a signed
action's JSON they are 32-element byte arrays (serde form).

| Method | Path | Returns |
|---|---|---|
| POST | `/v1/actions` | body: `SignedAction` JSON (`{envelope:{signer,nonce,chain_id,action},signature}`) → 202 `{tx_id, admitted:true}` or 422 `{admitted:false, code, error}` |
| GET | `/v1/status` | `{chain_id, height, timestamp, state_hash, validators[], mempool, accounts, orders}` |
| GET | `/v1/supply` | KEEL supply: `cap`, `issued`, `burned`, `block_reward` (`cap`, `minted`, `remaining`, `rate_per_year`, `proposer_bps`, `halving_ms`, `started_ms`, `next_halving_ms`) and `community_reward` (`cap`, `minted`, `remaining`, `keel_per_fee_usd`) |
| GET | `/v1/stable` | the unified USDT: `supply`, `reserves_in_stable`, `rebalance_pool`, `params` (`auto_convert`, `rebalance_max_premium_bps`), and per basket network `reserve`, `cap`, `target_ppm`, `share_ppm`, `premium_bps` (what a `WithdrawStable` through it pays now, and the rebate a deposit on it earns) |
| GET | `/v1/params` | every governance parameter (`docs/governance.md`): the scalar `Params`, nested `clients`/`liquidity`/`recovery`/`prices`/`stable`, and the dotted limit keys (`budget.activation_keel`, `sessions.max_secs`, …) flat |
| GET | `/v1/accounts/{addr}` | `{nonce, activated, budget{activation_keel, activation_balance, fee_mode, action_fee_keel, free_lock_min_keel, used,limit,cancel_limit,remaining,earned,locked,lock_cap_per_day,lock_pool,sponsor{client,left_today}|null,…}, tier, tier_expires_at, balances[{asset,account_type,balance}]}` |
| GET | `/v1/accounts/{addr}/orders` | `{open:[OrderRecord]}` |
| GET | `/v1/accounts/{addr}/trades` | `{trades:[Trade]}` |
| GET | `/v1/markets` | `{markets:[summary]}` |
| GET | `/v1/markets/{pair}` | summary: cfg, house quote, `last_price`, `reference_price` (time-weighted over `twap_window_secs`, what settlement reads; null when the window is 0), best bid/ask, volumes |
| GET | `/v1/markets/{pair}/book?depth=20` | `{bids:[{price,size,orders}], asks:[…], house, height}` |
| GET | `/v1/orders/{id}` | OrderRecord |
| GET | `/v1/offers?asset=&side=&fiat=` | `{offers:[Offer]}` (open offers only, each with `arbitrators[]`) |
| GET | `/v1/offers/{id}` | Offer, plus `arbitrators[]`: the bonded arbitrators the owner named (`SetOfferArbitrators`; empty = the elected set rules) |
| GET | `/v1/trades/{id}` | Trade (+ `dispute` when one exists), `arbitrator` (bound to rule it, or null) and `arbitrator_candidates[]` (copied from the offer at start) |
| GET | `/v1/vaults/{chain}` | `{chain, vault, state}` (vault registrations for the chain; whole vaults sub-state) |
| GET | `/v1/vaults/{chain}/addresses?from=0&limit=100` | deposit index → owner rows |
| GET | `/v1/vaults/outbounds?status=` | `{outbounds:[Outbound]}`; an outbound carries `tx_hash` (hex, 32 bytes) and `external_tx` (the id as the external chain writes it when `tx_hash` is only its hash: a Solana signature in base58, else `null`) |
| GET | `/v1/gov/proposals` | `{proposals:[Proposal], house_operator}` |
| GET | `/v1/gov/proposals/{id}` | Proposal |
| GET | `/v1/staking/validators` | `{consensus:[hex keys], staked:[{consensus_key,power}], staking}` |
| GET | `/v1/receipts/{tx_id}` | `{index, tx_id, signer, ok, error{code,message}?, events[]}` (404 until applied) |
| GET | `/v1/clients/{addr}` → `{address, attester, fee, caps, earned, usage_paid, usage_owed, attested_accounts, protocol_fee_waived, usage_prices{address_usd_micro, outbound_usd_micro, swap_usd_micro, fill_usd_micro, release_usd_micro, ruling_usd_micro}, billing_asset, locked_keel, tier, plan{tier, started_at, ends_at, active, suspended, fee_usd_micro, allowance_usd_micro, allowance_left_usd_micro, lock_keel, renew_tier, user_actions_per_day, user_actions_used_today}, user_actions_per_user_per_day, plan_period_secs, tiers[{tier, lock_keel, fee_usd_micro, allowance_usd_micro, user_actions_per_day}]}` | a client's retail schedule, its plan and tier, the usage prices and what it earned, paid and owes |
| GET | `/v1/treasury` → `{height, balances:{treasury,validator_rewards,burn}, last_buyback, params, fee_split_bps}` | system balances per asset and the epoch buyback record |
| GET | `/v1/recovery?status=&owner=` → `{height, params, recoveries:[{id, chain, evm_chain_id, token, to, owner, custodian, deposit_index, status, votes, tx_hash, external_tx, amount, created_height, closed_height}]}` | self-service recoveries of coins at a deposit address the chain cannot credit (`RequestRecovery`) |
| GET | `/v1/recovery/{id}` | one recovery row |
| GET | `/v1/liquidity` → `{height, active, pool:{balances, total_shares, value_usd, value_per_share, hwm, holders, protocol_shares}, params, last_epoch, pending_unlocks}` | the pooled house-maker inventory and its mark |
| GET | `/v1/liquidity/{addr}` → `{shares, share_bps, value_usd, legs, pending}` | one depositor's position and pending releases |
| GET | `/v1/ready/{chain}?max_checkpoint_age=` → `{chain, ready, reasons, height, vault, checkpoint, last_deposit_credited, outbound_pending, halted, fee_rate}` | whether a client may turn that network on; what a per-network switch polls |
| GET | `/v1/sync/meta` → `{height, state_hash, last_hash, schema, boundaries:[{epoch,height,digest}]}` | newest snapshot for state sync (`keel-node --sync-from`) |
| GET | `/v1/sync/snapshot` → bytes | the snapshot itself (`KEEL` header, schema, borsh state) |
| GET | `/v1/blocks/{height}/receipts` | `{height, receipts[]}` (last 10,000 blocks) |
| WS | `/v1/ws` | one JSON message per applied block: `{height, timestamp, state_hash, receipts[], events[], books{pair:{best_bid,best_ask,last_price}}}` |

Sub-states owned by other modules (offers, trades, vaults, proposals,
staking) are served as their serde JSON; field names follow the Rust
structs in `crates/keel-vm/src/modules/`.

Error codes on `/v1/actions`: `BAD_SIGNATURE`, `WRONG_CHAIN`, `BAD_NONCE`,
`NOT_ACTIVATED` (the address holds less than `budget.activation_keel` KEEL;
top it up, every other action is refused until then, cancels excepted; an
account whose client's plan can pay for the action, `budget.sponsor`, is
exempt),
`BUDGET_EXHAUSTED` (the address has no capacity: it must lock KEEL with
`LockBudget`; a lock of at least `budget.free_lock_min_keel` that it holds
in full is accepted without capacity), `BLOCK_CAP`, `PAUSED`, `UNAUTHORIZED`, `INVALID`. A
nonce may run up to 64 ahead of the chain nonce (pipelining); actions are
selected per signer in nonce order.

## Custody (client-owned vaults)

| Route | What |
|---|---|
| `GET /v1/custody` | one entry per **live generation** of every client-owned vault (active first): `custodian`, `chain`, `gen`, `status` (`Active`/`Draining`), `drain_until`, `reported_empty`, `vault` (that generation's key, chain code, epoch), `signer_url`, `address` (its index 0), `next_deposit_index`, `deposit_owners`, `reserves[]` (`asset`, `reserve`, `liabilities`, `halted`). Generations are separate pools: nothing is ever moved between them (`docs/models.md`, "Rotating to a fresh pool"); `RotateCustodyVault` with `sweep: true` and `ObserveCustodySweep` are refused with `INVALID` |
| `GET /v1/custody/{addr}` | one client's live generations and reserves, `rotations[]` pending (`RotateCustodyVault` with `activate_at`), the number of accounts in its custody, and `floats[]` (`asset`, `custody`, `deposit`: the custodian's balance on each side, what `SwapCustody` can draw on) |
| `GET /v1/custody/{addr}/{chain}/addresses?from&limit&owner&gen` | deposit addresses of that vault with their owners, from the active generation's key (`gen` picks a draining one); rows carry `gen` |
| `GET /v1/custody/{addr}/{chain}/addresses/lookup?address=` | which index, owner and generation a custody address belongs to, across every live generation |

Deposit rows (`/v1/vaults/deposits`) carry the same `tx_hash`/`external_tx`
pair. Outbound rows and batches under `/v1/vaults/outbounds` carry `custodian`
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
