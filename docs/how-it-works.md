# How the Keelchain works

*Technical paper, 2026-09-10. Describes the system as implemented in the
`blockchain` worktree on that date. Where something is designed but not
yet built, it says so.*

## Contents

1. What KEEL is, in one page
2. Consensus and validators
3. KEEL, issuance and the USD stablecoin
4. Governance: who holds which power
5. Fees
6. Transactions: how an action becomes state
7. Quota per address: budgets instead of gas
8. Wallets and keys
9. Funds and custody: vaults, observers, Lightning
10. Adding support for a new network
11. Markets: order book, house maker, and why there is no swap
12. Peer-to-peer offers, trades and disputes
13. The marketplace and its clients: what is the chain, what is a front end
14. Scenarios: a front end taken down; addresses labelled as tainted
15. Is it fully peer-to-peer and non-custodial?
16. Sensitive material and third parties
17. Known gaps

---

## 1. What KEEL is, in one page

KEEL is a Layer-1 blockchain that *is* an exchange. The state machine has
native modules for balances (a double-entry ledger), a central limit order
book, peer-to-peer fiat offers with escrow and disputes, custody vaults for
Bitcoin, Ethereum and Tron assets, a USD stablecoin, staking, governance,
attestations (KYC tiers) and, since this week, Bitcoin Lightning. There is
no virtual machine and no smart contracts: every operation is a typed
*action* with a fixed meaning, executed by every validator in the same
order.

Three properties shape everything else:

- **No gas.** An action is free to submit. Spam is bounded by a per-address
  *budget* that every account gets for free, that grows with the volume the
  address actually trades, and that can be enlarged by locking KEEL.
- **One ledger for everything.** Order fills, escrow, fees, withdrawals and
  rewards are balanced ledger postings. Nothing can go negative, nothing is
  created outside genesis except the stablecoin against its reserve.
- **The company is a client, not the system.** A marketplace client
  talks to the chain through the same RPC as any wallet. Clients that keep
  their own wallets are covered in `models.md`; this paper describes the
  network-vault model. It keeps operator
  keys for the roles the chain gave it (attester, parameter admin, treasury)
  and otherwise signs on behalf of users only while they still choose to
  leave a key with it.

The rest of this paper walks through each part and ends with the two
questions about resilience: what happens if a front end is
forced offline, and what happens if someone labels addresses as tainted.

## 2. Consensus and validators

### 2.1 How blocks are made

Blocks are ordered by **Simplex** consensus, using the Commonware
implementation (crates pinned at `2026.9.0`), adapted in the
`keel-consensus` crate. Certificates are plain Ed25519 signatures (no
distributed key generation). The leader for each view rotates round-robin
over the epoch's validator set. A block is final after two rounds of votes,
so finality is a quorum certificate, not a probability; the design
tolerates fewer than one third of validators being faulty or malicious.

Every validator executes every finalized block and the resulting state
commitment is chained: `last_hash ‖ height ‖ (tx_id, ok, error_code)* ‖
end-of-block events`. A node that diverges cannot follow the chain.

Blocks are paced by two chain parameters, both editable from the
backoffice: `min_block_interval_ms` (500 ms while there are pending
actions) and `idle_block_interval_ms` (5 s when the mempool is empty). The
network floor is two validator round trips; on the local testnet a block
takes about 22 ms when unpaced.

Nodes append each finalized block to a journal before touching state, take
a snapshot every 200 blocks (two newest kept) and restart from snapshot
plus journal replay. Full history is served from the journals; the block
explorer (`keel-indexer`) reads it into Postgres.

### 2.2 Epochs and the validator set

The staking module advances an epoch every `epoch_length_blocks`
(10,000). At the boundary the consensus engine for the next epoch is
started with the **top 100 addresses by power**, where power is a
validator's own bond plus the KEEL delegated to it; a jailed validator has
zero power. Ties break by address.

Fault tolerance follows the BFT rule `n = 3f + 1`: four validators tolerate
one faulty or offline validator, seven tolerate two; three tolerate none, so
a three-validator network halts when any one of them is down.

### 2.3 How someone becomes a validator

1. Hold KEEL in a chain account (the address is your Ed25519 public key).
2. Sign `Bond { role: Validator, amount, consensus_key }` with at least
   the minimum bond, `min_validator_bond` = 100,000 KEEL. The KEEL moves
   from your spendable balance into a restricted `stake_bond` account; it
   is still yours, just not spendable.
3. Run `keel-node` with the consensus key, the genesis file, the storage
   directory and a bootstrapper address, plus `--sync-from <rpc>` so the
   node starts from a peer's newest snapshot instead of replaying from
   genesis (`--sync-verify <rpc2>` makes a second peer confirm the tip
   hash). Commonware's authenticated discovery connects you to the set:
   a bonded key is tracked by every validator before it votes, and a node
   whose key is in no set runs as a follower (verifies and serves RPC,
   never votes) until governance's epoch includes it.
4. At the next epoch boundary, if you are in the top 100 by power, you
   propose and vote.

Anyone may `Delegate { validator, amount }` to a validator (the KEEL stays in
the delegator's own bond account and only adds to the validator's power).
`Unbond` and `Undelegate` queue a release after `unbonding_blocks`
(100,000 blocks); a partial unbond must leave at least the minimum. A
validator cannot self-delegate.

Two other bonded roles use the same action with a different `role`:
**observers** (`min_observer_bond` = 500,000 KEEL) and **arbitrators**
(50,000 KEEL). Bonding alone does not make you an observer or arbitrator:
membership of those sets is a governance decision (`SetObservers`,
`SetArbitrators`), because they hold custody shares and rule disputes.

### 2.4 How validators are paid

There is no block reward and no inflation. Every fee the chain collects is
split at collection time (section 5); the validators' share lands in a
system account `validator_rewards` **in the asset the fee was paid in**
(KUSD, BTC.BTC, …). At each epoch boundary that balance is distributed:
observers first take `observer_reward_bps` (25%) split equally, then the
remainder goes to validators pro rata to power, with each validator's
delegators receiving their pro-rata slice automatically. There is no claim
step.

Slashing parameters exist (`slash_double_sign_bps` 5%,
`slash_false_observation_bps` 100%) and the slashing routine is written
(burns the bond, jails the validator), but **no code path calls it yet**.
Until it is wired, misbehaviour is punished only by governance removing
the address from a set. This is listed in section 17.

## 3. KEEL, issuance and the USD stablecoin

### 3.1 KEEL

- Hard cap **21,000,000,000 KEEL**, six decimals. The whole supply is
  issued at genesis; nothing mints KEEL afterwards.
- Genesis buckets (basis points of supply, must sum to 100%): DAO treasury
  35%, community rewards 25% (the KEEL awards the platform already promised
  are paid from here at genesis, line by line), team 15% (vesting with a
  one-year cliff then linear over four years, released every block; the
  ungranted part waits in `team_reserve`), validator bootstrap 10%,
  liquidity 10% (the house maker's inventory account `swap_pool`), strategic
  reserve 5%.
- Nothing leaves a bucket without a governance proposal, except vesting,
  which the chain releases on schedule.
- Demand for KEEL comes from bonds (validator, observer, arbitrator),
  governance weight, the refundable 10 KEEL deposit per live P2P offer, the
  1,000,000 KEEL proposal deposit, buying action budget, and locking KEEL for
  action capacity (nothing is paid, the KEEL comes back after three days).
- Supply shrinks through the burn share of every fee (10% by default),
  vetoed proposal deposits and, once wired, slashed bonds.

I still have to decide the team grant list and the mainnet address
of the parameter admin; both are genesis inputs.

### 3.2 KUSD

The unit of account is a stablecoin minted 1:1 against a governance-set
basket of vaulted USD tokens (launch: USDT on Ethereum and Tron), held in
the chain's own `stable_reserve` accounts. Each basket entry has a cap and
an enable flag (`SetStableBasket`). Minting moves the reserve token into
the reserve and issues KUSD; burning does the reverse and is refused if
that asset's reserve cannot cover it. Conversions floor, so rounding can
only leave reserve behind, never issue unbacked coin. Mint and burn charge
no fee. The reserve is visible at every block. A crypto-collateralized mint
path is planned but not built.

Note for the testnet: the marketplace currently maps its "USDT" balance to
native KUSD (`CHAIN_USDT_ASSET=KUSD`), which is why the explorer shows
KUSD for offers created in USDT. That mapping is an open decision.

## 4. Governance: who holds which power

### 4.1 Proposals

Anyone with 1,000,000 KEEL can `Propose`. The deposit is locked; voting runs
for `voting_period_blocks` (20,000), then a timelock of 5,000 blocks, then
anyone may `ExecuteProposal`. A vote's weight is the voter's bonded KEEL (all
three roles) plus delegations received; unbonding KEEL does not vote; a
re-vote replaces the earlier one.

A proposal passes when turnout is at least 33.4% of all bonded KEEL, fewer
than 33.4% of the votes cast are *veto*, and *yes* is at least 50% of
yes-plus-no. A vetoed proposal burns its deposit; passed and rejected ones
refund it. If execution fails the ledger is rolled back and the proposal is
recorded as failed, never retried.

Proposal kinds: change a parameter; list or delist a trading pair; register
an asset; spend from the treasury; schedule a software upgrade (version and
height at which unupgraded nodes halt); set the arbitrator, attester or
observer sets (with the observer threshold); set a stablecoin basket
entry; pause a module until a height; bind a token contract; set the
Bitcoin or Ethereum light-client checkpoint; set or revoke the parameter
admin; or a text proposal.

### 4.2 The parameter admin

One administrative key exists: `param_admin`, an optional address set at
genesis and changeable only by a `SetParamAdmin` proposal. It may sign
`SetParam { key, value }` for any of the roughly fifty scalar parameters
and the seven `budget.*` keys, with the same validation a proposal would
get (fee split must sum to 100%, block pacing bounds, and so on). It cannot
move funds, change sets, pause modules or upgrade software.

At launch this key is the platform super admin's, held by the marketplace as
`CHAIN_ADMIN_SECRET`, and the backoffice **Chain** page is its user
interface: every parameter, grouped (block production, fees, P2P, staking,
governance, vaults, Lightning, markets, action budget), with a description
and a live status strip. Governance can take the right away at any time by
passing `SetParamAdmin { admin: None }`, after which only proposals move
parameters.

### 4.3 What nobody can do

- Freeze, seize or edit a balance. There is no such action.
- Mint KEEL.
- Move funds out of a vault without an observer quorum signing.
- Change a parameter silently: every `SetParam` and every proposal is a
  transaction with a tx id, shown in the explorer under Governance.

The strongest emergency levers are non-discretionary and asset-scoped: the
per-block reserve invariant halts outbounds of an asset whose observed
reserves fall below user liabilities, and governance can `PauseModule`.

## 5. Fees

Every number is a chain parameter with a default in `keel-vm::Params`:

| Fee | Default | Paid by | Collected at |
|---|---|---|---|
| Book taker | 10 bps | taker, out of what it receives | each fill |
| Book maker | 0 | — | — |
| P2P release | 1%, plus 1% when the trade is under $50 | seller (escrowed at trade start) | `ReleaseTrade` |
| Dispute | 1% of the trade | losing side, capped at its share | `RuleDispute` |
| Withdrawal flat fee | $1 in the withdrawn asset | withdrawer | `Withdraw` |
| Withdrawal network cost | median observer-reported rate × size | withdrawer, pass-through, overpayment refunded | on confirmation |
| Lightning routing allowance | max(0.5%, 10 sats), unspent part refunded | withdrawer | on settlement |
| Stablecoin mint / burn | none | — | — |
| Extra action budget | 0.001 KEEL per action | buyer | `BuyBudget` |

Every fee is one balanced posting split into the system accounts
`treasury` (50%), `validator_rewards` (40%) and `burn` (10%); rounding
dust goes to the treasury. The split must always sum to 100%. USD values
for the P2P surcharge and withdrawal fee come from the chain's own
`<ASSET>-KUSD` order book, not an external oracle.

The marketplace charges its own withdrawal fee schedule on top (the
tiered flat fee on the backoffice Fees page). Note: that fee is USD
pegged and dominates small Lightning withdrawals; Lightning deserves its own
tier if the goal is "cheap".

## 6. Transactions: how an action becomes state

An action travels in an **envelope**: `signer (32 bytes) ‖ nonce (u64) ‖
chain_id (u32) ‖ borsh(action)`. The signer is an Ed25519 public key and
*is* the address (64 hex characters, no prefix). The digest is
`sha256("keel-action-v1" ‖ envelope)`, the signature is Ed25519 over it,
and the transaction id is `sha256("keel-txid" ‖ digest ‖ signature)`. The
TypeScript SDK and the Rust crates produce byte-identical results (30
shared test vectors).

Admission, in order: chain id, signature, signer is not the system address,
nonce is exactly the next one for that key, session-key resolution, module
not paused, then either observer authorization (bonded, budget-free
actions) or one unit of budget. Everything after that runs inside a ledger
savepoint: if the module refuses (insufficient funds, bad price), the
savepoint rolls back, the nonce and one budget unit are still consumed, and
a failure receipt is recorded. Blocks therefore never become invalid
because of a user's mistake, and replay is impossible.

A receipt carries height, timestamp, tx id, signer, ok/error and the list
of typed events (`Transferred`, `OrderFilled`, `TradeReleased`,
`DepositCredited`, `LightningPayoutSettled`, …). The explorer materialises
these into balances, transfers, offers, trades, orders and parameter
history; the marketplace records the tx id of every action it signs
against the entity it belongs to, so every offer, trade, withdrawal, lock
and migration step in the site and the backoffice links to the scanner.

The mempool is per-signer and nonce-ordered, keeps 64 nonces of lookahead,
drops actions after ten minutes, and never gossips an action from an
address whose budget is exhausted. A proposal holds at most 5,000
actions. Signature checks run in parallel; verdicts are consumed in order.

### Session keys

A user can `AuthorizeSessionKey { key, scope, expires_at }` for another
Ed25519 key with a scope bitset: `MARKETS` (place/cancel orders, house
quotes) and `P2P_MANAGE` (create/update/pause/close offers, mark paid). A
session key has its own nonce but spends the principal's budget and acts
as the principal. It can **never** transfer, withdraw, start, release or
cancel a trade, lock budget, bond, vote, attest or manage sessions: those
actions have no scope. At most 16 live keys per principal, 30 days
maximum, revocable by either side. This is what lets the marketplace trade
for a non-custodial user without a wallet popup per click.

## 7. Quota per address: budgets instead of gas

Every address has a **budget**: `limit = base + earned`, with
`budget.base` = 10,000 free actions. Each action, successful or not,
spends one unit; observers' attestations spend none. `earned` grows by one
action per whole USD the address fills on an KUSD-quoted pair (taker or
maker) and by purchase (`BuyBudget`, 0.001 KEEL per action, routed through
the fee split). Cancels (cancel order, close offer, cancel trade) draw on
a separate ceiling `min(limit + 100,000, 2 × limit)` so a capped address
can always unwind. A per-block cap (`budget.max_per_block` = 200) bounds
burst.

The free budget does not regenerate. Capacity for heavy users comes from
**locking KEEL** (the Tron energy model, decided 2026-09-08): each
whole KEEL locked grants `budget.per_locked_keel_per_day` (100) actions per
day into a pool that refills linearly over 24 hours; the pool is spent
after the free budget. `UnlockBudget` returns the KEEL in full after
`budget.unlock_delay_secs` (3 days). Nothing is paid, so it adds demand for
KEEL without adding a fee. The wallet page shows the capacity card and the
backoffice user page shows each user's lock.

Other per-address caps: at most 500 open orders (currently counted across
pairs, see section 17) and a P2P offer allowance that starts at 2 live
offers and grows by 2 for every 10 completed trades.

When a budget is exhausted the chain answers `BudgetExhausted`; when the
per-block cap is hit, `BlockCap`. The marketplace shows both as "action
capacity" errors with the lock card as the remedy.

## 8. Wallets and keys

### 8.1 Two modes

**Non-custodial (target state, decided 2026-09-09).** The user's
key lives in the KEEL browser extension (`apps/wallet-extension`): a BIP39
24-word seed, keys derived as `secret_0 = seed[0..32]` and
`secret_i = sha256(seed ‖ u32_le(i))`, encrypted at rest with PBKDF2
(310k) + AES-GCM under the wallet password. The extension injects
`window.keel` (connect, signMessage, signAction, authorizeSession); every
signature is shown decoded and approved by the user. It talks to the node
RPC the user configured and never to the marketplace. The marketplace, when
it needs a user signature, creates a *signing request* (in memory, 180 s),
the site relays it to the extension, the signature comes back and is
verified against the stored envelope before use. Login and sign-up can use
the wallet as the credential (a signed challenge with the `keel-message-v1`
domain, which can never be confused with an action).

**Custodial (legacy, until each user migrates).** For accounts created
before the wallet existed, the marketplace generated an Ed25519 key per
user and stores it AES-256-GCM-encrypted under `CHAIN_KEY_ENC_SECRET` in
Postgres. The migration flow moves every balance to the user's own address
(one `Transfer` per asset, signed by the custodial key), re-attests the KYC
tier for the new address, retires the cached deposit addresses and deletes
the encrypted secret. Preconditions: no open offers, trades, orders or
unlock queue on the custodial address; the site lists what to close.

### 8.2 Who can see what

| Party | Holds |
|---|---|
| User with the extension | the only copy of the private key |
| Marketplace (wallet-mode user) | nothing for that user except scope-limited session keys it generated, which the chain refuses for anything that moves funds |
| Marketplace (custodial user) | the encrypted key; whoever holds `CHAIN_KEY_ENC_SECRET` and the database can sign as that user, which is why I chose to eliminate this mode |
| Validators | public keys, signatures and balances only; they never see a private key |
| Observers | their own chain key and a *share* of the vault key (section 9) |

Address format: 64 hex characters, the raw Ed25519 public key; there is no
prefix or checksum, so `bc0c…` is not a Bitcoin address, just an address
that happens to start with those characters.

## 9. Funds and custody: vaults, observers, Lightning

This section describes the network vaults: the model for clients whose users
hold Keel Wallet. A client that keeps its own wallets registers its own vault
and signs its own withdrawals; where the funds sit in each model, who signs,
and what the explorer shows is in `models.md`.

### 9.1 Vaults and threshold keys

Assets from other chains live in **vaults**: one active vault per external
chain, registered with an epoch number, a 33-byte secp256k1 public key, a
chain code, the list of signers and a threshold. The private key never
exists: it is generated as a **CGGMP21 threshold ECDSA key** whose shares
sit with the bonded observers' signer daemons (passphrase-encrypted share
stores). Signing a withdrawal needs the threshold; no observer can do it
alone and no server holds the key.

Per-user deposit addresses are **non-hardened BIP32 children** of the
vault key at path `m/<slip44>/<index>` (index 0 reserved for the vault's
own hot address, users from 1). Any node can compute a child *public* key
from the registered public key and chain code, so deposit addresses are
derived on the fly by the RPC; the signers fold the derivation into the
threshold signature without any party learning a child secret.

### 9.2 Deposits

An observer that sees a payment to a derived address submits
`ObserveDeposit` with the chain, tx hash, output index, deposit index,
amount, external height and a **proof**:

- Bitcoin: block headers and a Merkle inclusion proof, checked by every
  validator against the chain's own SPV header chain (proof of work and
  retargets verified), 2 confirmations.
- Ethereum: a receipt proof against the sync-committee light client and
  the registered token contract, 12 confirmations.
- Tron: no practical light client exists, so the observation is accepted on
  quorum alone, 19 confirmations.

Where a light client exists a proof is mandatory. The credit happens when a
quorum of distinct *current* observers (`observer_threshold`, a count set
by governance with the set) has voted. Deposits above $50,000 sit in a
`screening_hold` account for 600 blocks and are then released
automatically; there is no discretionary hold. One credit per
`(chain, tx_hash, index)`.

### 9.3 Withdrawals

`Withdraw { asset, to, amount }` moves the amount plus a network-fee lock
into the user's `sendout_escrow` and collects the flat fee. Every 20
blocks the chain batches queued outbounds per chain; an observer leads the
signing of each batch (`batch_id mod signers`, with the next signer taking
over after a timeout), Bitcoin batches become one transaction with a
one-in-flight guard, Ethereum and Tron pay each outbound from the hot
address. Observers report the result (`ObserveOutbound`) and the quorum
settles on the majority verdict and the median reported fee; the escrow
is reconciled, overpaid fee refunded. Failure refunds in full.

### 9.4 The reserve invariant

At every block boundary, for every vault asset, the chain checks that the
system's observed reserves are at least the sum of user liabilities. A
breach emits `InvariantBreached` and halts outbounds of that asset; a
parameter change cannot clear it, only an explicit resume. Lightning pools
(next section) are counted inside the same invariant.

### 9.5 Bitcoin Lightning (added 2026-09-10)

Lightning needs a hot node with channels, which a chain cannot run, so each
observer may run an LND node and register it. The chain keeps a **pool**
per observer: BTC lent to that node from the vault (`FundLightningPool`, a
normal quorum-confirmed outbound), pending outgoing payments and today's
credits. Deposits: the marketplace asks the observer's invoice API for an
invoice whose description binds the owner's address; on settlement the
observer reports the preimage and every validator re-parses the invoice,
checks the payee is that observer's node, the preimage matches the payment
hash, and credits the owner. Withdrawals: a `Withdraw` whose destination is
a BOLT11 invoice locks the amount plus a routing allowance
(`max(0.5%, 10 sats)`), is assigned to the observer with the most free
pool, and is refunded if the observer misses a 600-block deadline. Pools
are capped (0.5 BTC each by default), single deposits and payouts at 0.1
BTC, all editable on the Chain page. Excess is swept back to the vault via
an announced transaction. The hot exposure per observer is therefore
bounded by the cap and backed by the observer's 500,000 KEEL bond.

Verified on the local testnet through the site: a deposit credited in
about 8 seconds, a payout settled in 4 seconds.

## 10. Adding support for a new network

Today the set of external chains is a closed enum in the code: Bitcoin,
Ethereum, Tron (Solana was explicitly dropped). Adding a fourth is a
**software change** (chain adapter in `keel-chains`, a light client if the
chain has one, observer support for watching and broadcasting), shipped
through a `SoftwareUpgrade` proposal that names a version and a height at
which unupgraded nodes halt. Once the code knows the chain, everything else
is governance and observer actions:

1. `RegisterAsset { asset, decimals }` (and `SetTokenContract` for tokens).
2. `SetBtcCheckpoint` / `SetEthCheckpoint`-style checkpoint for the light
   client, if any.
3. Observers run a key generation and submit `RegisterVault { chain, epoch,
   public_key, chain_code, signers, threshold }`; the highest epoch is the
   active vault.
4. `ListPair` to open an `ASSET-KUSD` book, and, if it is a USD token,
   `SetStableBasket` to admit it as reserve.
5. The marketplace maps its currency/network names to the chain asset
   (`mapping.ts`) and, for Lightning-like rails, points at observer APIs.

Rotating a vault (new observer set) is the same `RegisterVault` with a
higher epoch followed by moving balances as ordinary withdrawals; there is
no automatic per-epoch rekey yet.

## 11. Markets: order book, house maker, and why there is no swap

The order book is a native module with a pure matcher (`keel-book`): limit
and market orders, price-time priority, the taker pays the maker's price,
self-trade prevention, tick and lot alignment, minimum and maximum
notional, at most 500 open orders per owner. One `PlaceOrder` action locks
funds, matches against the book, settles every fill through the ledger
(base, quote, fee, release legs) and rests the remainder. A fill on an
KUSD-quoted pair also earns both sides budget. Pairs are listed and
delisted by governance; the devnet lists `BTC-KUSD`. The VM settles
roughly 156,000 orders per second on one core.

**House maker.** A pair may have a house party (by default the system
`swap_pool`, the 10% liquidity bucket). The address governance designates
as house operator, or the party's owner, signs `HouseQuote { pair, bid,
ask, valid_until }`; the quote appears as synthetic levels clamped to the
house inventory and expires after at most 60 blocks. This is how the
platform provides a price from block one without a market-making bot
having to post real orders.

**Swap.** There is no automated market maker or swap module on chain. The
`swap_pool` name is the house maker's inventory account, not a
constant-product pool. The marketplace's "swap" feature is an off-chain
desk that fills from the book (through the house maker) and settles as
ordinary fills; from the chain's point of view it is a taker order with a
tx id like any other. An on-chain AMM would be a new module and a
governance-listed pool; it is not on the roadmap.

## 12. Peer-to-peer offers, trades and disputes

An **offer** publishes side, asset, fiat currency, payment-method label,
margin (±50% max) or fixed price, min/max amount, payment window (≥ 5
minutes), country, minimum KYC tier, terms and a hash of the payment
instructions. Sell offers must be fundable when posted (balance ≥ max
amount; recorded, not locked). Each live offer holds a refundable 10 KEEL
deposit; the allowance ladder in section 7 limits how many an address may
run. Update, pause and close are actions with tx ids; the marketplace
mirrors its offers on chain so trades can escrow there.

A **trade** (`StartTrade`) moves amount plus the seller fee from the
*seller's* spendable balance into the seller's restricted
`marketplace_escrow` account: escrow is a transfer, not a lock, so it
shows in the ledger as such. States: Funded → Paid (buyer's `MarkPaid`
with a proof hash) → Released (seller's `ReleaseTrade`: escrow to buyer,
fee split) or Cancelled. The buyer may cancel before paying; after the
payment window expires, **anyone** may cancel an unpaid trade, so a stuck
trade never needs a user's or the operator's key. Chat and payment details
stay off chain, end-to-end encrypted, with only hashes committed.

A **dispute** can be opened from Paid by either party (the buyer after a
one-hour grace). Up to 32 evidence hashes can be attached. Any member of
the governance-elected, bonded arbitrator set may `RuleDispute` with
WinsSeller, WinsBuyer or a split; the dispute fee comes from the loser's
share. There is no marketplace override key. Per-dispute assignment and the
ruling window are not yet enforced (section 17). On the current testnet the
arbitrator set is the platform's key; on mainnet it is whoever governance
elects.

**KYC tiers** enter the chain only as attestations: an attester address
set by governance signs `Attest { subject, tier, expires_at }`. The only
thing a tier gates on chain is starting a trade against an offer that
requires `min_tier`. Withdrawal and trade limits by tier are marketplace
policy, not chain rules.

## 13. The marketplace and its clients: what is the chain, what is a front end

**Inherent to the chain** (works with any client, survives any single
operator): balances, the order book and matching, offers, trades, escrow,
disputes and rulings, vault deposits and withdrawals, the stablecoin,
Lightning pools, staking, governance, attestations, budgets, session keys,
the explorer data model (anyone can run `keel-indexer` against a node).

**A marketplace client** is one kind of client. It runs the KYC'd
storefront, fiat payment instructions, chat, notifications, email, support,
moderation (including an off-chain address blacklist that refuses
*withdrawals* to listed addresses and freezes the account for review; it
cannot block a deposit or a chain transfer), the swap desk, referral and
award programs, and the backoffice. On the chain it holds four operator
roles, each a plain chain key: attester (tiers), parameter admin (fees and
limits, revocable by governance), treasury (revenue accounts, referral
payouts), and house operator (quotes). It also holds, temporarily,
custodial keys for unmigrated users and scope-limited session keys for
migrated ones.

**Other clients** that exist today: the browser wallet extension (signs
anything, reads balances from the node), the `keel` CLI (every action,
including staking, governance and Lightning operations), the block
explorer (read-only), and the TypeScript SDK on which the site and the
extension are built. A different company could ship a competing front end
against the same chain tomorrow; it would need its own attester
registration from governance if it wants its KYC tiers recognised.

## 14. Scenarios

### 14.1 A jurisdiction forces one front end offline

Suppose an order shuts down a client's web app and its API in one
country, or entirely.

*Who is affected.* Users of that front end lose the storefront, chat,
notifications, fiat payment instructions display, and the moderation and
support they were used to. Users still in **custodial** mode also lose the
ability to sign, because their key is stored by the operator: their funds
are intact on chain and visible on any explorer, but nobody can move them
until the operator, or someone with the encrypted keys and
`CHAIN_KEY_ENC_SECRET`, signs. This is the exposure the non-custodial
migration removes; every custodial user is a liability in this scenario,
and the site nags them to migrate for that reason.

*Who is not.* Users in **wallet** mode keep every balance and can act
through the extension, the CLI, or any other client: cancel orders, close
offers, release or cancel trades, withdraw to Bitcoin, Ethereum, Tron or a
Lightning invoice. Open trades unwind by the rules in section 12; an
unpaid trade past its window is cancellable by anyone, a paid one can be
released by the seller or disputed to the arbitrators. Validators keep
producing blocks; observers keep crediting deposits and paying withdrawals
because they are bonded participants, not employees of the front end.

*What degrades.* Attestations: no new tiers are issued while the attester
is down, so offers requiring a tier stop admitting new counterparties;
existing attestations run until expiry. Parameters: nobody adjusts fees
until governance passes proposals or revokes the admin. House quotes stop,
so the book relies on organic makers. Disputes: on a testnet where the
platform is the only arbitrator, rulings stop; on mainnet governance should
elect independent arbitrators before launch for exactly this reason.

*Mitigations already in the design.* Multiple front ends are first-class
(the extension and CLI are complete clients); the explorer and indexer are
runnable by anyone; observer and arbitrator sets are governance-elected and
bonded; the parameter admin is revocable; the marketplace's keys are ordinary
chain keys that governance can replace by re-electing sets. Two mitigations
remain to do: finish migrating every custodial user, and elect an
arbitrator and attester set that is not a single company.

### 14.2 Certain addresses get labelled as tainted

Suppose an analytics vendor, an exchange or a regulator labels some chain
addresses, or some Bitcoin addresses that deposited into the vault, as
tainted.

*On the chain itself, nothing happens.* There is no freeze, blacklist or
seizure action; the reserve invariant and module pause are the only halts,
and both are asset- or module-wide, never per address. A validator cannot
refuse to include a valid action without forking off the chain (it would
disagree with the state hash). Balances, offers and trades of a labelled
address keep working.

*Where a label bites.* At the edges: an external exchange may refuse a
withdrawal from the vault's hot address if it considers the vault
tainted; a fiat counterparty may refuse a P2P trade; the KYC'd marketplace
may, by its own policy, add the address to its withdrawal blacklist (which
only stops *that front end* from sending to it) or decline to attest a
tier. Because the vault is one pooled key per chain, a taint claim against
the vault's Bitcoin address is a claim against every user's deposit
address at once, which is the main systemic risk.

*Mitigations.*

- Keep taint decisions off chain and per client, which is how it is built:
  the marketplace's blacklist and screening are its own, and other clients
  need not share them. Users of a labelled address keep self-custody and
  can leave through any client.
- Per-vault epochs: governance can register a fresh vault (new key, new
  observer set) and migrate clean balances, isolating a contested key.
- Large deposits already pause for 600 blocks; the same hook could route
  flagged deposits to a governance-visible hold, but that would be a
  discretionary freeze and I have so far chosen not to have one.
  Adding it would be a proposal and a code change, not a config flag.
- Attestations are the correct place for compliance signals: a tier is a
  positive claim by a named attester with an expiry, which lets offers
  require it without the chain judging anyone.

*Who is affected.* Only the labelled addresses' counterparties who choose to
honour the label, and, in the vault-level case, everyone until governance
rotates the vault. Validators and honest users elsewhere are untouched.

## 15. Is it fully peer-to-peer and non-custodial?

Precisely:

- **On-chain assets (KEEL, KUSD, trades, orders, escrow): non-custodial
  once the user is in wallet mode.** The key is in the user's extension; the
  chain enforces that nothing else can move the balance. Escrow is held by
  the chain's rules, not by an operator. Trades are peer to peer with
  arbitrators only on dispute.
- **Bridged assets (BTC, ETH, USDT on Tron/Ethereum): custodied by a
  federation, not by the user.** The vault key is a threshold key held by
  the bonded observer set; deposits need a quorum and a light-client proof,
  withdrawals need a quorum signature. This is the ZetaChain/THORChain
  model: no single custodian, no company key, but a set of bonded operators
  whose collusion above the threshold could move funds. The reserve
  invariant makes any shortfall visible and halts payouts. Lightning adds
  bounded hot exposure per observer.
- **Custodial-mode users: not yet.** Until they migrate, the marketplace
  can sign for them. The design's answer is to make that mode disappear.
- **Peer discovery and matching: on chain.** Offers and the order book are
  chain state; no server is needed to find a counterparty, though the
  marketplace is the friendliest place to do it.
- **Fiat: inherently off chain.** The fiat leg of a P2P trade is between the
  two people; the chain sees a proof hash and an escrow.

So: fully non-custodial for chain-native value and for anyone who has moved
to their own wallet; federated custody with on-chain proof for bridged
value; and a marketplace that is a client, not a gatekeeper.

## 16. Sensitive material and third parties

### 16.1 Secrets that exist, and where

| Secret | Held by | Consequence of loss or theft |
|---|---|---|
| User seed phrase / extension vault | the user | loss = funds unrecoverable; theft = full control of that address |
| Validator consensus key | each validator | theft allows double-signing (slashing planned) |
| Observer chain key (`KEEL_OBSERVER_SECRET`) | each observer | theft allows false observations until the quorum and proofs stop them; bond at stake |
| Vault key share (CGGMP21) | each observer's signer daemon, passphrase-encrypted | one share is useless alone; threshold shares move the vault |
| LND wallet and macaroon | observers running Lightning | exposure bounded by the pool cap |
| `CHAIN_ADMIN_SECRET` (parameter admin) | a marketplace client | can change fees and limits within validation; revocable by governance |
| `CHAIN_ATTESTER_SECRET` | a marketplace client | can issue tiers; governance can replace the attester |
| `CHAIN_TREASURY_SECRET` | a marketplace client | controls platform revenue accounts |
| `CHAIN_KEY_ENC_SECRET` + database | a marketplace client | decrypts remaining custodial user keys; the reason for the wallet migration |
| Session keys (server-generated) | a marketplace client | scope-limited; cannot move funds; expire in ≤ 30 days |
| Genesis file | public | none; it is the chain's public starting state |

Chat content and payment instructions are end-to-end encrypted between
counterparties; the chain and the marketplace database hold hashes. KYC
documents never touch the chain.

### 16.2 Third parties the system relies on

| Dependency | Used for | If it fails |
|---|---|---|
| Commonware (consensus, p2p, storage, Rust crates) | block ordering and networking | isolated behind `keel-consensus`; Malachite is the planned fallback |
| CGGMP21 (`cggmp21` crate) | threshold ECDSA for vault keys | key ceremony is offline; a bug is a code fix and a vault rotation |
| Bitcoin Core (pruned, watch-only) | observers watching and broadcasting BTC | observers stall; deposits/withdrawals pause, funds safe |
| reth + Lighthouse | Ethereum watching, sync-committee proofs | same |
| Tron Lite FullNode | Tron watching | same; Tron has no light client so it depends on the observer quorum entirely |
| LND | Lightning pools | pool operations pause; on-chain BTC unaffected |
| Postgres | indexer/explorer read model; marketplace data | explorer stale; chain unaffected |
| Google Fonts, CDNs, hosting, email | front ends only | front-end degradation, section 14.1 |
| Tatum | the legacy custody gateway that the chain replaces | retired at cut-over |

There is no price oracle: USD values come from the chain's own book. There
is no KMS dependency on chain; the marketplace's own KMS discipline applies
to its operator keys.

## 17. Known gaps (specified but not built)

- Slashing: the routine exists, nothing calls it; `slash_*` parameters are
  unread.
- Referral share (`referral_share_bps`) is a TODO in the P2P release path.
- Observer quorum uses the governance-set count; `observer_quorum_bps` is
  not applied.
- `deposit_daily_cap_usd_micro` is not enforced.
- Disputes: no per-dispute arbitrator assignment; `ruling_window_secs`
  unused; any elected arbitrator may rule.
- `max_open_orders_per_pair` is applied per owner across pairs.
- No crypto-collateralized stablecoin path; no on-chain swap/AMM.
- Vault rotation and observer-set changes are manual governance steps, not
  an automatic per-epoch rekey.
- Validator disk: mainnet needs pruning and BLS certificates before a public
  validator set; a node keeps full journals today.
- Mainnet items not started: external audits, key ceremony, public
  testnets, hosted explorer, the team grant list and super-admin address.

Everything above is in `docs/` alongside the plan, the tokenomics,
the wallet contract, the Lightning design and the testnet manual, and the
numbers quoted are the defaults in `chain/crates/keel-vm/src/params.rs` on
2026-09-10.
