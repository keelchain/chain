# Building a client on Keel

A client is any service that signs actions for its users or holds a role on
the chain: an exchange, a marketplace, a payments desk, a bot. This guide is
the integration contract: how a client runs its own ledger and custody next
to Keel, turns Keel on one network at a time, and reconciles everything by
transaction id. `models.md` explains the two models (custodial client with
its own vaults, P2P client on Keel Wallet); this page is the mechanics, and
it is the same for both.

New here? `exchange.md` ("Run an exchange on Keel: getting started") is
the step-by-step path through Keel Harbor; this page is the contract
behind it.

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
| Where it manages its account | Keel Wallet | Keel Harbor ([harbor.keelchain.com](https://harbor.keelchain.com/)): plans, payment, usage, lock, fees and vaults, each change an action signed in the wallet |

## What is stable

`https://testnet.keelchain.com/rpc/v1` (node) and `/api/v1` (indexer) are
the contract. Within `v1` routes and fields are only added, never removed or
renamed; a deprecation is announced thirty days ahead in the changelog and
kept working meanwhile. The reference pages are `rpc`, `api` and `ws` under
keelchain.com/docs, and `@keelchain/sdk` on npm tracks them.

## Accounts and roles

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

- **One Keel account per user, or one aggregate account.** An account is
  an Ed25519 public key. A custodial client derives and stores its users'
  keys in its own KMS and signs as custodian (*transparent*: one chain
  account per user, visible on the explorer), or keeps a single aggregate
  account and its per-user balances in its own database (*rails only*;
  `models.md`). Either way the client submits signed actions to `POST
  /v1/actions` and reads receipts.
- **Onboarding a user is three steps:** `Attest` the account, send it the
  KEEL for its lock (1 KEEL covers a retail user), and have its first
  action be `LockBudget`, which your service signs since it holds the key.
  Until it has locked, every action from it but a cancel is refused with
  `BUDGET_EXHAUSTED` (or `NOT_ACTIVATED` while it holds less than
  `budget.activation_keel`, 0.1 KEEL). While your plan runs neither the
  lock nor the activation KEEL is needed: the account's actions come out
  of your plan's daily pool (Pricing). Then `RequestCustodyAddress` with
  the account as `custodian`'s owner gives it an address in your vault.

</div>
<div class="tab" data-mode="noncustodial">

- **One Keel account per user.** An account is an Ed25519 public key, and
  a P2P client's users hold theirs in Keel Wallet: the address your site
  learns when the wallet connects is the account you attest. The client
  submits what the wallet signs (or what its session key signs) to `POST
  /v1/actions` and reads receipts.
- **Onboarding a user is three steps:** `Attest` the account, send it the
  KEEL for its lock (1 KEEL covers a retail user), and have the user lock
  it from the wallet's *Action capacity* card, or sign the `LockBudget`
  through your site. Until it has locked, every action from it but a
  cancel is refused with `BUDGET_EXHAUSTED` (or `NOT_ACTIVATED` while it
  holds less than `budget.activation_keel`, 0.1 KEEL). While your plan
  runs neither the lock nor the activation KEEL is needed: the account's
  actions come out of your plan's daily pool (Pricing), so a retail user
  with no KEEL at all can act the moment you attest them. The KEEL is the
  user's and comes back when they unlock.

</div>
<div class="tab" data-mode="user">

- **One Keel account, yours.** Generate a key in Keel Wallet; the address
  is the public key, and there is nothing to register. Nobody attests it
  unless you join an exchange, and no operator account is involved.
- **Getting started is two steps:** hold at least 1 KEEL (the faucet on
  the testnet, a transfer otherwise) and lock it from the wallet's *Action
  capacity* card. A `LockBudget` of at least 1 KEEL is accepted from an
  account with no capacity, so the first lock needs nothing but the KEEL.
  Until then every action but a cancel is refused with `BUDGET_EXHAUSTED`
  (or `NOT_ACTIVATED` while the address holds less than 0.1 KEEL). The
  lock is a hundred actions a day and comes back three days after you
  unlock.

</div>
</div>

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

Governance can change these numbers (`docs/governance.md`); read
`budget.free_lock_min_keel`, `budget.per_locked_keel_per_day` and
`budget.activation_keel` from `GET /v1/params` at startup rather than
hard-coding them.

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

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

1. `ready` is true, and your custody vault for the chain is registered
   (`RegisterCustodyVault`; `GET /v1/custody/{you}` lists it).
2. The operator account has budget (locked or bought KEEL).
3. One address issued in your vault for a test account
   (`RequestCustodyAddress`), and a small deposit credited
   (`deposits:<chain>:<addr>` on the socket, or `GET /v1/vaults/deposits?owner=`).
4. One small custody withdrawal to an address you control, signed by your
   signer and confirmed (`WithdrawalQueued` → `OutboundBatched` →
   `OutboundConfirmed`).
5. One float swap each way (`SwapCustody`) for the test account, and the
   `CustodySwapped` event seen on `account:<addr>`.
6. The reconciliation job below has run clean for a day.

</div>
<div class="tab" data-mode="noncustodial">

1. `ready` is true.
2. The operator account has budget (locked or bought KEEL).
3. One deposit address issued for a test account (`RequestDepositAddress`),
   and a small deposit credited (`deposits:<chain>:<addr>` on the socket, or
   `GET /v1/vaults/deposits?owner=`).
4. One small withdrawal to an address the client controls, confirmed
   (`WithdrawalQueued` → `OutboundBatched` → `OutboundConfirmed`).
5. One escrow round trip (`StartTrade`, `MarkPaid`, `ReleaseTrade`), signed
   from a test user's Keel Wallet through your site.
6. The reconciliation job below has run clean for a day.

</div>
<div class="tab" data-mode="user">

There is no switch for you to flip. The wallet's Receive card offers a
network when the chain has a vault for it, and `GET /rpc/v1/ready/{chain}`
is the same answer the wallet reads. If a network is not ready, a new
deposit address or a withdrawal on it waits; what you already hold is not
at risk.

</div>
</div>

## Where the funds sit: your vault, or the network vault

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

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

USDT in your custody is one asset, `USDT`, whichever network it came in
on: a deposit of Tron, BNB Smart Chain or ERC-20 USDT to one of your
addresses is credited as `USDT` and stays in your vault as its backing.
Pay it out with `WithdrawCustodyStable { via, to, amount }` on a network
where your vault holds enough; `GET /v1/custody/{addr}` reports the
backing per network under `stable` (`models.md`, "One USDT in custody").

To settle a trade between two of your own users without leaving custody,
move the balance with `TransferCustody`: seller to your escrow account,
then to the buyer on release or back on cancel, signed by you
(`on_behalf_of`) with your own reference in `memo`. A transfer takes up
to 16 legs in one atomic posting, so the buyer and your fee, affiliate
and referral accounts are paid together or not at all. No float, no
swap, no usage price; the `CustodyTransferred` event reaches both
accounts' streams (`models.md`, "Transfers inside custody").

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

</div>
<div class="tab" data-mode="noncustodial">

A P2P client holds no vault. Its users' funds sit in the **network
vault** for each chain, a threshold key held as shares by the bonded
observer set (`how-it-works.md` §9), and the client uses the ordinary
actions: `RequestDepositAddress` per account (the wallet asks for the same
address when the user presses Receive), `Withdraw` and `WithdrawStable` for
payouts, signed by the user in Keel Wallet, and `StartTrade`, `MarkPaid`,
`ReleaseTrade` for escrow. Balances land in the `deposit` account type and
trade on the chain's book and in escrow with everyone else's, swapped-in
custodial balances included.

The reserve check for the network vault is chain-wide: `GET
/v1/vaults/{chain}` shows the vault, its epoch, signers, checkpoint and
whether it is halted, and the explorer's vault page is every user's proof
of reserves. Your reconciliation ingests `WithdrawalQueued`,
`OutboundBatched`, `OutboundConfirmed` and `DepositCredited` on
`account:<addr>` for each user you manage; there is no custody row, no
float and no rotation on your side. A key rotation of the network vault is
a governance step (a `RegisterVault` with a higher epoch, announced ahead;
`how-it-works.md` §10), and the new key means a new address string at
every account's unchanged index, so fetch the deposit address at deposit
time (`GET /v1/vaults/{chain}/addresses?owner=`), never from a stale
cache.

</div>
<div class="tab" data-mode="user">

Your coins in Keel sit in the network vault, the threshold key the bonded
observer set holds; nobody at Keel and no client can sign for it alone. The
wallet's Receive card asks the chain for your address on each network
(`RequestDepositAddress`), Send withdraws through the observers' batch, and
the explorer's vault page shows the reserve against every balance it backs.
You may also keep Bitcoin or Tron **in your wallet**, under a key from your
own seed, outside the chain, and move them in only to trade (`wallet.md`
§5b). If an exchange holds your balance in its own vault, Keel Wallet shows
that too: the address in the exchange's custody, a withdrawal through its
signer, and the swap onto the chain's book and back (`wallet.md` §5c).

</div>
</div>

## Your public profile

Set the name your exchange is listed under with `SetClientProfile { name,
website, logo }` (Harbor, Business, Public profile). It is what people see
in the explorer's Exchanges directory and what an API consumer reads from
`GET /api/v1/clients`.

- **The name is yours alone.** The chain refuses a name another client
  holds, in any spelling: `Acme Exchange`, `acme-exchange` and
  `ACME.EXCHANGE` are one name. 3 to 32 letters, digits, single spaces,
  dots and hyphens. Changing your name frees the old one.
- **The website is proved, not claimed.** Give the host name only
  (`acme.example`), then serve a text file containing your client address
  at `https://acme.example/.well-known/keel.txt`, over HTTPS and without a
  redirect. The read API fetches it every ten minutes and reports
  `website_verified`; until then the directory shows the website as not
  verified, and the logo is not loaded.
- **The logo** is an `https://` link to a square image, optional.

Every offer your users post is listed under you. The chain writes your
address into the offer (`client`) when it is created: the owner's own
address when the owner is a client, otherwise the client whose unexpired
attestation the owner holds. The owner does not choose it and cannot
change it, so nobody outside your users can post under your name. Anyone
can then ask for your offers alone:

```
GET /api/v1/offers?client=<your client address>&status=active
```

The full list of what an offer, a trade and an account make public is in
`public-data.md`.

An offer keeps the client it was created under. Offers that live only in
your own database are not on the chain and are not listed; to be found on
Keel, an offer has to be posted with `CreateOffer`.

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

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

Only trades on the `deposit` side reach chain arbitration: a custody
balance trades on the chain's book or escrow only after the float swap
(`models.md`). A trade between two of your own custody users never touches
the chain and stays with your own support, as today.

</div>
<div class="tab" data-mode="noncustodial">

Every trade of your users is a chain trade, so the above applies in
full: name your team on every offer your site creates, and decide which
other arbitrators you accept when your users take other makers' offers.

</div>
<div class="tab" data-mode="user">

You do not bond an arbitrator; you choose one by taking an offer. The
wallet and the explorer show `arbitrators[]` next to an offer's price (an
exchange's offers name that exchange's dispute team); taking it accepts
them, and when an offer names several you pick one with `ChooseArbitrator`
before `MarkPaid`. An offer that names nobody is ruled by the chain's
elected set.

</div>
</div>

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

## The event log

`GET /api/v1/events?address=<your operator address>&cursor=` on the
indexer is every event that names your account, oldest first: custody
credits (held or not), transfers and swaps of your users, payouts from
the moment they are queued to `OutboundConfirmed` or `OutboundFailed`
(both name the account and you), and the network fee
each one cost you (`CustodyNetworkFeePaid`: the figures for a gas report),
plan periods, usage charged and owed. It includes the events a block emits by itself (a plan
renewal, usage collected at an epoch boundary), which no transaction
lists. Read a page, apply it, keep `next_cursor`, ask again; at the tip
the same cursor returns what has arrived since, so a balance mirror is a
loop over this one route. `type=` narrows it to one event type and
`from_height=` starts later than genesis. Use the streams below when you
need to react within a block, and this to be sure you missed nothing.

## Streams

Subscribe rather than poll for anything time-sensitive: `account:<addr>`
for a user's receipts, `deposits:<chain>:<addr>` for credits, `pair:` and
`book:` for markets. Every frame has a sequence number and the server tells
a client when it fell behind (`gap`); the SDK's `KeelSocket` reconnects and
replays from the last height. Details in the `ws` reference.

## Signing

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

Sign server-side with `@keelchain/sdk` (`Keypair`, `RpcClient.send`), keys
in your KMS. Two keys, with two jobs:

- **Your operator key** for what a person decides: roles, fees, the plan,
  the lock, payouts from your own balance. It is the key you connect to
  Harbor and use in the CLI.
- **A session key for your service**, authorized once by the operator key
  (`AuthorizeSessionKey`, scope `client_ops`, bit 4; `keel send session
  authorize <key> --scope client-ops`). It signs the day-to-day work for
  your account: `Attest`, `RequestCustodyAddress`, `TransferCustody`,
  `SwapCustody` and `ReleaseCustodyHold`. The operator approves it in
  Harbor (Business, Service keys); your service can hand them a link that
  fills the form in, so no address is copied:
  `https://harbor.keelchain.com/testnet/business/keys?key=<address>&scope=12&name=<your service>`.
  It has **its own nonce**, so an action signed in Harbor
  or the CLI can never take a nonce your service already signed for, and
  it cannot buy a plan, change fees or pay out. Payouts
  (`WithdrawCustodyFor`) are a scope of their own, `client_payouts`
  (bit 8): give it to the same key (scope 12) or to a separate one that
  only sends coins out, as your own controls require. A key expires
  (`sessions.max_secs`, 30 days at most); renew it before then, or revoke
  it with `RevokeSessionKey` if the service is compromised.

**Your users need no keys.** An account on Keel is an address, and for a
custodial user nobody has to hold a key for it: you attest the address,
ask for its deposit address yourself (`RequestCustodyAddress` with
`owner`), and move its balance yourself (`on_behalf_of`). Derive the
address from your own customer id however you like, deterministically,
and there is no seed to back up and nothing to lose: the mapping can be
recomputed, and the chain lists every account in your custody
(`GET /v1/custody/{addr}/{chain}/addresses`). A user who should sign for
themselves keeps a real key in Keel Wallet instead. To pay a user out,
send `WithdrawCustodyFor { owner, asset, via, to, amount, fee }`: one
action that pays out of the user's account through your vault (`via` names
the network when the asset is `USDT`). `fee: { to, amount }` charges your
own withdrawal fee on top, to an account in your custody, in the same
posting: the payout is queued and the fee taken together, or neither.

**Screening deposits before they can be spent.** By default a deposit is
credited straight to the user's balance. Send `SetCustodyOptions {
hold_deposits: true }` once (operator key) and every deposit to one of
your users is credited **on hold** instead (`CustodyDepositHeld`, with the
external transaction hash): it is in your vault's reserve and counted as
owed, and nobody can spend, transfer or withdraw it. After screening,
`ReleaseCustodyHold { owner, asset, amount, to }` moves it to the user's
balance, or with `to` to another account in your custody when the user is
not to have it. A release can be partial. Your own top-ups are never held.
The node does not report who sent a deposit; look the senders up on the
external chain from the transaction hash.

Withdrawals from your vault are signed by your own signer under the same
policy every Keel signer runs: the transaction must pay open outbounds of a
finalized batch, and nothing else.

</div>
<div class="tab" data-mode="noncustodial">

Ask the user's Keel Wallet (`window.keel.signAction`) and submit what
comes back; use a **session key** (`AuthorizeSessionKey`, scope `markets`
or `p2p_manage`) so trading and offer management do not need a popup per
click. Session keys can never move funds. `wallet.md` has the provider
contract and `connect-keel-wallet.md` the integration. Your operator key
signs only your own actions: attestations, fees, the plan, your treasury.

</div>
<div class="tab" data-mode="user">

Keel Wallet signs everything: each action is shown decoded in the
wallet's own window and approved by you before it leaves the page, and a
site you connect to only ever receives signatures, never the key. The CLI
(`keel send … --secret`) and the SDK (`Keypair`, `RpcClient.send`) sign
with the same key for anyone who prefers a terminal.

</div>
</div>

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
| Withdrawal queued | $0.10 |
| Float swap between custody and the market | free |
| Order of one of your users that filled | $0.002 |
| Escrow release | $0.05 |
| Dispute ruling | $1 |

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

The usage you incur: addresses issued in your vault and custody
withdrawals, and, for balances swapped onto the chain's book, your users'
fills and releases. The float swap itself is free. A custody-to-custody transfer inside your own
ledger is not a chain action and costs nothing.

</div>
<div class="tab" data-mode="noncustodial">

The usage you incur: deposit addresses the network vault issues to your
users, their withdrawals, and the fills, releases and dispute rulings of
their trades. Your own operator actions cost nothing beyond your lock.

</div>
<div class="tab" data-mode="user">

None of this table is yours. A wallet user buys no plan and is charged no
usage; the protocol fees apply instead: 10 bps on a fill you take, 1% on
an escrow release and the flat fee on a withdrawal
(`how-it-works.md` §5, `governance.md`). Actions cost nothing beyond the
1 KEEL you lock, which comes back when you unlock. Join an exchange and,
while its plan runs, the protocol fees are waived and its retail fee
applies instead.

</div>
</div>

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
- **You buy it in Keel Harbor.** [harbor.keelchain.com](https://harbor.keelchain.com/) is the
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
