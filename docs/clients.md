# Building a client on Keel

A client is any service that signs actions for its users or holds a role on
the chain: an exchange, a marketplace, a payments desk, a bot. This guide is
the integration contract: how a client runs its own ledger and custody next
to Keel, turns Keel on one network at a time, and reconciles everything by
transaction id. `models.md` explains the two models (custodial client with
its own vaults, P2P client on Keel Wallet); this page is the mechanics, and
it is the same for both.

## A client is not a user

The chain treats the two differently in one respect, how they pay. If you
are a person with a wallet, the left column is all that applies to you and
the rest of this page is not for you.

| | User | Client (an exchange) |
|---|---|---|
| What it is | Any address: a person with Keel Wallet who trades, posts an offer or moves coins for themselves | An address that governance registered as an **attester**: a business that puts its own users on the chain and vouches for their KYC tier |
| How it becomes one | Generates a key. Nothing to register | A `SetAttesters` governance proposal names its operator address |
| What it locks | 1 KEEL or so, for its own action capacity. Returned on unlock. Nothing at all if a client with a plan attested it: the plan includes actions for the client's users | The KEEL its plan tier requires, on its operator account (100,000 / 500,000 / 2,000,000 KEEL): one half of the membership, and capacity for that account. Returned on unlock |
| What it pays to act | Nothing. No gas, no fee per action | Nothing per action either |
| What it pays on value moved | The protocol fees: 10 bps on a fill it takes, 1% on an escrow release, $1 on a withdrawal | A membership: the plan fee of its tier every 30 days on top of the lock, and a dollar price per flow of its users, in USDT. Its users then pay no protocol fees, and it charges them its own |
| Who its counterparty is | The chain | The chain; its users are accounts it attested |
| Where it manages its account | Keel Wallet | Keel Harbor (`harbor.keelchain.com`): plans, payment, usage, lock, fees and vaults, each change an action signed in the wallet |

## What is stable

`https://testnet.keelchain.com/rpc/v1` (node) and `/api/v1` (indexer) are
the contract. Within `v1` routes and fields are only added, never removed or
renamed; a deprecation is announced thirty days ahead in the changelog and
kept working meanwhile. The reference pages are `rpc`, `api` and `ws` under
keelchain.com/docs, and `@keelchain/sdk` on npm tracks them.

## Accounts and roles

- **One Keel account per user.** An account is an Ed25519 public key. A
  custodial client derives and stores its users' keys (or keeps a single
  aggregate account, see `models.md`); a P2P client's users hold theirs in
  Keel Wallet. Either way the client submits signed actions to `POST
  /v1/actions` and reads receipts.
- **One operator account per client**, holding the roles governance granted
  it: **attester** (it stamps a KYC tier on its users' accounts with
  `Attest`, which the chain reads for limits) and, when it runs its own
  vault, the **vault owner**. The `Onboard client` workflow gives a testnet
  client its roles and a starting balance of KEEL and USDT; on mainnet the
  same goes through a governance proposal. `GET /v1/gov/roles` shows the
  current sets.
- **Capacity, not gas.** No action costs a fee, and no account has a free
  budget: an account acts on the capacity of the KEEL it has locked
  (`LockBudget`: 100 actions a day per KEEL, the first day's worth at
  once, the KEEL back in full three days after `UnlockBudget`). That holds
  for your operator account and for every user account that signs. A
  `LockBudget` of at least 1 KEEL is accepted from an account with no
  capacity, so the first lock needs nothing but the KEEL. Capacity is
  linear in KEEL and additive across addresses, so spreading work over
  many accounts does not change what you lock in total. `GET
  /v1/accounts/{addr}` shows `budget.remaining`, `budget.locked` and
  whether the account is `activated`.
- **Onboarding a user is three steps:** `Attest` the account, send it the
  KEEL for its lock (1 KEEL covers a retail user), and have its first
  action be `LockBudget`. Until it has locked, every action from it but a
  cancel is refused with `BUDGET_EXHAUSTED` (or `NOT_ACTIVATED` while it
  holds less than `budget.activation_keel`, 0.1 KEEL). The KEEL is the
  user's and comes back when they unlock; if you hold the user's key or a
  session key, your service signs the lock. Governance can change these
  numbers (`docs/governance.md`); read `budget.free_lock_min_keel`,
  `budget.per_locked_keel_per_day` and `budget.activation_keel` from `GET
  /v1/params` at startup rather than hard-coding them.

## The per-network switch

Turn Keel on one external network at a time. The switch lives in the
client: when `bitcoin` is on, **new** deposit addresses, **new** withdrawals
and **new** escrow for Bitcoin go through Keel; everything that started
before stays on the old custody path until it completes; nothing is moved
between the two. The client's ledger stays the client's; Keel's balances are
the Keel side of it.

Before turning a network on, and continuously afterwards, read

```
GET /rpc/v1/ready/{BTC|ETH|TRON}
```

It answers `ready: true` only when the network's vault is registered, the
light-client checkpoint is fresh (Bitcoin), nothing is halted, and it lists
the reasons otherwise, plus the last deposit credited, the pending outbounds
and the fee rate. A client flips its switch off when `ready` goes false and
back on when it recovers; no funds are at risk in between, only new
operations wait.

Checklist for one network:

1. `ready` is true.
2. The operator account has budget (locked or bought KEEL).
3. One deposit address issued for a test account (`RequestDepositAddress`),
   and a small deposit credited (`deposits:<chain>:<addr>` on the socket, or
   `GET /v1/vaults/deposits?owner=`).
4. One small withdrawal to an address the client controls, confirmed
   (`WithdrawalQueued` → `OutboundBatched` → `OutboundConfirmed`).
5. For a P2P client: one escrow round trip (`StartTrade`, `MarkPaid`,
   `ReleaseTrade`).
6. The reconciliation job below has run clean for a day.

## Custodial clients: your own vault

A client that keeps its own keys (docs/models.md, Model A) uses the custody
actions instead of the network vault: `RegisterCustodyVault` once per chain
(public key, chain code, signer URL), `RequestCustodyAddress` per account,
`WithdrawCustody` per payout. The observers attest deposits on the client's
addresses like any other; the client's signer signs its batches after the
same policy check the network signers run. Balances land in the `custody`
account type, separate from network-backed balances. `GET /v1/custody/{addr}`
is the client's reserve versus liabilities per asset; the per-network switch
applies the same way (enable a network once its custody vault is registered
and `/v1/ready/{chain}` is green for deposits).

To let a custody balance trade on the chain's book or escrow, swap it
against your own floats with `SwapCustody` (`to: "deposit"`), and swap it
back (`to: "custody"`) when the user is done: one posting, no external
transaction, both reserve checks untouched (`models.md`, "The float
swap"). Keep a `deposit` float of your own for the way in and a custody
float (index-0 top-ups) for the way out; `GET /v1/custody/{addr}` reports
both as `floats[]`, and the `CustodySwapped` event on `account:<addr>` is
what your reconciliation ingests. Rebalance the floats with an ordinary
`WithdrawCustody` to your own network deposit address, or a `Withdraw` to
your vault's index-0 address.

To rotate your vault key for security (a scheduled rekey, a signer
migration, a suspected compromise, a new custody provider), start a
second signer for a new key and `RotateCustodyVault` (`models.md`,
"Rotating to a fresh pool", and "What changes and what does not" there
for the two meanings of "address": your Keel accounts, revenue and treasury
included, never change; the external deposit strings do; the old pool keeps
crediting its addresses and paying withdrawals until it is empty, and
nothing is ever moved from the old key to the new one: generations are
separate pools, by protocol rule, not by option).
Nothing on your side changes except the addresses you show: fetch a user's deposit address from
`GET /v1/custody/{you}/{chain}/addresses?owner=` **at deposit time**, never
from a cache older than the session, and subscribe to `PoolRotated` on
`account:<you>` to refresh what is on screen. Old addresses keep
crediting during the grace, so a user mid-deposit is safe; a reverse lookup
of any live generation's address is
`GET /v1/custody/{you}/{chain}/addresses/lookup?address=`. Keep the old
signer reachable until `GET /v1/custody/{you}` shows that generation
`Retired`.

## Naming your dispute team

Disputes on your users' trades are ruled by whoever the offer names. Bond
your dispute team's address as an arbitrator (`Bond { role: Arbitrator }`,
at least `min_arbitrator_bond`; no election needed), then have every offer
your site creates name it with `SetOfferArbitrators` (one address binds at
`StartTrade`; several let the taker choose with `ChooseArbitrator` before
`MarkPaid`, else the first applies). Takers accept the arbitrator by taking
the offer, so show it next to the price. When your users take other
makers' offers, filter on `arbitrators[]` of `GET /v1/offers` to decide
which arbitrators you accept; an offer that names nobody is ruled by the
chain's elected set. Your team rules with `RuleDispute` on
`GET /v1/trades/{id}` whose `arbitrator` is its address; the
`TradeArbitratorBound` event on `account:<arbitrator>` tells it which trades
it holds. Governance can ban an arbitrator chain-wide; its bond is what it
has to lose.

## Idempotency and reconciliation

Every signed action carries the signer's **nonce**; the chain applies each
nonce once and in order, so the nonce is the idempotency key. A client keeps
a table

```
client_id -> (signer, nonce, tx_id, status)
```

and computes `tx_id` locally before submitting (`txId(signedAction)` in the
SDK, the hash of the signed envelope), so a crashed process can look the
receipt up (`GET /v1/receipts/{tx_id}`, or the indexer's `/v1/txs/{tx_id}`
for anything older than the node's recent window) instead of signing again.
A duplicate submit of the same signed bytes is rejected as a nonce reuse and
is harmless. Free-text `memo` fields on transfers are for people, not for
matching.

Reconciliation: once a day, for every Keel account the client manages,
compare `GET /v1/accounts/{addr}` balances (deposit, escrow, sendout
escrow) with the client's ledger, and every outbound of the day with the
indexer's `/v1/vaults/outbounds`. The explorer shows the same numbers to the
client's users.

## Streams

Subscribe rather than poll for anything time-sensitive: `account:<addr>`
for a user's receipts, `deposits:<chain>:<addr>` for credits, `pair:` and
`book:` for markets. Every frame has a sequence number and the server tells
a client when it fell behind (`gap`); the SDK's `KeelSocket` reconnects and
replays from the last height. Details in the `ws` reference.

## Signing

- Custodial client: sign server-side with `@keelchain/sdk` (`Keypair`,
  `RpcClient.send`), keys in the client's KMS.
- P2P client: ask the user's Keel Wallet (`window.keel.signAction`) and
  submit what comes back; use a **session key** (`AuthorizeSessionKey`, scope
  `markets` or `p2p_manage`) so trading and offer management do not need a
  popup per click. Session keys can never move funds. `wallet.md` has the
  provider contract.
- Withdrawals from a client-owned vault are signed by the client's own
  signer under the same policy every Keel signer runs: the transaction must
  pay open outbounds of a finalized batch.

## Pricing

A client pays a membership and usage. The membership is a plan: a fixed
fee every 30 days plus KEEL kept locked, both required. The fee and the
usage are priced in dollars and paid in USDT from the operator account's
deposit balance. There is no invoice: you buy the plan in Keel Harbor,
the chain takes each charge when it falls due, and turns what it collects
into KEEL on its own order book. Nothing is a share of your own fees.

| Tier | KEEL locked on the operator account | Plan fee per 30 days | Usage included | Actions a day for your users |
|---|---|---|---|---|
| 1 | 100,000 | $300 | $300 | 10,000 |
| 2 | 500,000 | $1,000 | $1,200 | 50,000 |
| 3 | 2,000,000 | $3,000 | $4,000 | 250,000 |

| Usage | Price |
|---|---|
| Deposit address issued to one of your accounts | $0.02 |
| Withdrawal queued, or a float swap | $0.10 |
| Order of one of your users that filled | $0.002 |
| Escrow release | $0.05 |
| Dispute ruling | $1 |

All of these are governance parameters (`docs/governance.md`); `GET
/v1/clients/{addr}` returns the live tier table and prices with your
`tier` (the highest your lock meets), `plan` (`tier`, `ends_at`, `active`,
`suspended`, `renew_tier`, `allowance_left_usd_micro`,
`user_actions_used_today`), `usage_paid` and `usage_owed`. Harbor is that
endpoint with buttons.

- **A plan is the fee and the lock together.** Neither alone is a plan:
  the lock without the fee is only action capacity, and the fee cannot be
  paid without the lock. Lock the tier's KEEL on the operator account
  with `LockBudget` (the same lock that gives the account its action
  capacity); it is yours and comes back in full three days after
  `UnlockBudget`.
- **You buy it in Keel Harbor.** `https://harbor.keelchain.com` is the
  client's account: connect the operator account's wallet, pick a tier,
  and sign one action, `BuyPlan { tier }`. The chain checks the lock,
  takes the tier's fee in USDT from the operator account and starts the
  period in that block (`ClientPlanStarted`); there is nothing to wait
  for. Harbor has no server and no account of its own: it reads the chain
  and every change is an action you sign, so the CLI (`keel send
  buy-plan <tier>`) or the SDK do exactly the same.
- **Upgrading.** While a period runs you can buy a higher tier once the
  lock meets it. A fresh period starts at once, and what the old one had
  left comes off the fee: its fee times the share of the time left, or
  the share of the allowance left when that is smaller.
- **Renewal is automatic.** When a period ends the chain starts the next
  one in the same block, at the tier you chose, so there is no gap.
  `SetPlanRenewal { tier }` changes that tier (a lower one is how you
  downgrade) and 0 stops renewal. If the lock no longer meets the tier
  the plan renews at the highest one it does meet. If the account cannot
  pay the fee there is no plan until it can, and the first block in which
  it can renews it; the fee is never carried as debt.
- **The plan counts while the lock is in place.** Unlock below the
  running tier's KEEL and the plan is suspended from that block, with no
  refund; lock it back and what is left of the period returns.
- **While a period runs, your users pay no protocol fee** on fills, escrow
  releases, dispute rulings and the withdrawal flat fee, and you set your
  own retail fee (`SetClientFee`, uncapped), which the chain pays into your
  account in the same posting. Without a plan you are still an attester,
  but your users pay the protocol fees like anyone else.
- **Your users need no KEEL.** Each tier includes a daily pool of actions
  for the accounts you attested. A user's own capacity is used first, if
  it has any; after that its actions come out of your pool, up to 100 a
  day per user (`clients.user_actions_per_user_per_day`), and such an
  account needs neither a lock nor the activation KEEL. When the pool or
  the user's share is used up for the day, the user is an address like
  any other until the next day (UTC) or until it locks KEEL of its own.
  The account endpoint shows it as `budget.sponsor`.
- **Usage draws on the allowance first** (`UsageCovered`), then on your
  USDT (`UsageCharged`). A deposit address or a withdrawal you cannot pay
  for is refused. A fill, release or ruling is never blocked: what you
  cannot pay is owed (`UsageOwed`), collected from your next USDT, and
  until it is paid no new address or withdrawal is issued to your
  accounts, your users pay the protocol fees again and your pool pays
  for none of their actions.
- **It is consensus, not a service.** The tier table, your period, your
  allowance, your users' pool and what you owe are chain state, and every
  charge is part of executing the block: the purchase, the renewal, and
  the address request, withdrawal, fill, release or ruling that incurs a
  usage price. Every validator computes the same result or the block does
  not finalize. Nobody meters you from outside, no validator or operator
  can waive, raise or skip a charge, and Harbor cannot either: prices
  move only by a `ParamChange` proposal, and you can audit every charge
  as an event (`ClientPlanStarted`, `ClientPlanRenewalSet`,
  `UsageCovered`, `UsageCharged`, `UsageOwed`) and a ledger posting.
- **What to keep on the operator account:** the tier's KEEL locked, and
  USDT for the next plan fee plus the usage you expect beyond the
  allowance.

## Testnet policy

The testnet resets when the genesis has to change (a validator set change,
a schema change that cannot migrate). Resets are announced in the changelog
and re-onboard every client listed with the workflow; balances are testnet
balances. `GET /rpc/v1/status` carries the chain id and the state hash a
client can pin.

## Getting help

mohab@keelchain.com for roles, API access and partnerships;
support@keelchain.com for everything else.
