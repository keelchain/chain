# Two ways to run an exchange on Keel

For the steps to get started (Keel Harbor, registration, the lock and the
plan) see `exchange.md`; this page is about who holds the funds.

Keel is infrastructure. An exchange, a marketplace, a payments desk or a bot
is a client of the chain, and every client chooses one of two models per
network (Bitcoin, Ethereum, Tron, BNB Smart Chain, Solana). Both models use
the same API, the same explorer, the same SDK and the same readiness
endpoints; they differ in one question: **who holds the funds and who signs
the withdrawals.** A person who connects Keel Wallet to the chain directly,
with no exchange in between, is the third reader of this page: the last
column below, and the *Wallet user* tab in every section that has one.

| | Model A: custodial client | Model B: P2P client | A wallet user, no client |
|---|---|---|---|
| Who holds user funds | The client, in **its own vault**: keys it owns, a signer it runs | Keel's network vault for that chain, threshold-signed by the observer set; escrow on chain | The network vault, the same as Model B; or the user's own key on the external chain (`wallet.md` §5b) |
| What Keel does | Detects deposits to the client's addresses, keeps the ledger, builds and batches withdrawals, estimates fees, broadcasts, records everything on the explorer | Everything in A, plus on-chain escrow, disputes, the order book and session keys | Everything in B |
| Who signs withdrawals | The client's own signer | The observer set (2-of-3 on the testnet) | The observer set |
| Who signs user actions | The client, as custodian | The user, with Keel Wallet | The user, with Keel Wallet |
| What users can verify | Proof of reserves for that client: vault addresses, reserve against the sum of balances, every movement with a transaction id | Their own account and every action they signed | Their own account and every action they signed |
| Client fees and margin | **Untouched.** Keel charges usage, never a share of the client's fees | **Untouched.** While the client's plan runs the chain waives its protocol fee for the client's users; the client sets its own retail fee and receives it in the same posting, and pays usage per fill, release, ruling and withdrawal | No client, no retail fee: the user pays the protocol fees (`how-it-works.md` §5) |
| How Keel is paid | A plan every 30 days and usage, priced in dollars and paid in USDT, which the chain buys into KEEL; plus KEEL the client locks and gets back | The same way | The protocol fees, and 1 KEEL locked for action capacity, returned on unlock |

A client can run both models at once: custodial accounts in its own vault,
wallet users on the network vault. It can also switch a network from one
model to the other with its own per-network setting; old balances stay where
they are, new deposits follow the new setting, and nothing has to be moved.

## The model, in detail

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

### Model A: the custodial client

This is for an exchange that already runs its own wallets and wants to keep
doing so. Today such an exchange pays a custody provider for address
generation, deposit webhooks, signing and broadcasting, and it has no way to
show its users what it holds. On Keel it keeps its keys and gets the rest.

**What the client does**

1. Generates its vault key and keeps it. A single key, a hardware signer, its
   existing KMS, or a threshold set of its own: the chain does not care. The
   client runs `keel-tss serve --policy-rpc <node>` on its own machine (the
   same signer the network observers run, with the signing policy that
   checks every request against the chain's outbound rows), or puts a small
   signing hook in front of its KMS: `POST /sign` with `{digest, path,
   context}` answering `{r, s, recovery_id}`, exactly what `keel-tss serve`
   answers. The URL must be reachable by the observers that build its
   batches (a public HTTPS endpoint behind the client's own allow-list).
2. Registers the vault on chain with `RegisterCustodyVault`: chain, epoch,
   the compressed public key, the chain code and the signer URL. Only an
   attester (a client onboarded by governance) can register, and only its
   own vaults; a higher epoch rotates the key.
3. Requests deposit addresses in its vault with `RequestCustodyAddress`
   (`chain`, `custodian`): the client itself for an aggregate account, or
   any account the client attested, one address per user. Index 0 is the
   vault's own address: the client's own top-ups of gas and float land
   there and belong to the client (a payout's own change returning there
   is not a deposit; the observers tell the two apart by the inputs).
4. Withdraws with `WithdrawCustody`; the chain escrows the custody balance,
   batches the vault's outbounds separately from the network's, an observer
   builds the transaction, the client's signer signs it after the policy
   check, and the observers broadcast and confirm it. Network fees come out
   of the client's own vault: from its gas balance at index 0 when it has
   one, otherwise booked as its expense until its next top-up settles it.

Custody balances are a separate account type (`custody`), never mixed with
balances backed by the network vault: a custody balance cannot be traded on
the chain's order book or withdrawn through the network vault, and an account
belongs to one custodian. That separation is what makes the per-vault reserve
check exact. The way across is the float swap below.

### The float swap: custody to the market and back, no coins moving

A custodial client can hold a float on both sides: coins in its own vault
(its `custody` balance, topped up at index 0) and a `deposit` balance in the
network vault (its own address from `RequestDepositAddress`). A user's
balance changes model by swapping against those floats in one ledger
posting, `SwapCustody { asset, amount, to }`:

```
to deposit   user custody -X   client custody +X
             client deposit -X user deposit  +X
to custody   the four legs reversed
```

Nothing moves on Bitcoin, Ethereum or Tron. The client's vault still backs
the same total (the user's custody balance became the client's), and the
network vault still backs the same total of `deposit` balances, so both
reserve checks are untouched. Once on the `deposit` side the balance trades
on the chain's book and escrow with everyone else, wallet users included,
and comes back the same way. The user signs the swap, or the client signs
it for an account in its custody (`on_behalf_of`); an attested account that
never asked for a custody address joins the client's custody by swapping
into it. The swap is refused while either vault is halted for the asset or
when the client's float on the far side is short.

### Transfers inside custody: your own users' trades, on chain

Two users of the same custodial client do not need the float swap to
trade with each other. `TransferCustody { to, asset, amount, on_behalf_of,
memo }` moves a custody balance from one account to another inside the
same client's custody, in one ledger posting:

```
seller custody -X   recipient custody +X
```

The client's vault backs the same total before and after, so its reserve
check is untouched, nothing is sent on any network, there is no float to
hold in the network vault, and the transfer is not billed. That makes the
chain the client's ledger for its own users' trades as well, with each leg
an event anyone can audit:

1. **Escrow.** The seller's balance moves to an escrow account of the
   client's: the client's operator account, or a separate account it
   attested for that purpose. The client signs for the seller
   (`on_behalf_of`), or the seller signs it in Keel Wallet.
2. **Release or cancel.** The client moves the balance from its escrow
   account to the buyer, or back to the seller. `memo` carries the
   client's own reference (a trade id and the leg) and is echoed in the
   `CustodyTransferred` event.

Who may sign: the owner of the balance, or its custodian. Where it may go:
the custodian itself, an account already in its custody, or an account it
attested, which joins its custody by receiving. It never leaves the
client: a transfer to an outsider or to another client's user is refused,
and that case is what the float swap above is for.

This is the client's own escrow, run by the client: the payment window,
the dispute and its ruling are the client's, as they are on its ledger
today. The chain's escrow with elected arbitrators applies to trades on
the `deposit` side.

### Rotating to a fresh pool

A custodial client rotates its vault key on a chain for security: a
scheduled rekey, a signer migration (a new `keel-tss` set, a new KMS), a
suspected compromise of the old key or of its signer, or a change of
custody provider. The rotation moves its whole platform to a fresh vault
on that chain without touching a single Keel account: `RotateCustodyVault { chain, public_key, chain_code, signer_url,
activate_at }`. The client makes the new key on a signer of
its own (a second `keel-tss serve`, or its KMS behind a second `POST /sign`
URL; the old signer keeps serving the old key), and registers its public
key and chain code; no private key is ever sent, at registration or at
rotation. From the block it activates, or at `activate_at`:

- every account gets a **new deposit address** on that chain, derived from
  the new key at its unchanged index (the index is what the chain stores;
  `GET /v1/custody/{client}/{chain}/addresses` hands out the active key's
  addresses, `?gen=` an older generation's);
- the old key becomes a **draining generation**: still watched, a deposit
  to an old address still credits the same account, and withdrawals keep
  being paid from it, one key per transaction, until it is empty;
- after `pools.grace_blocks` the observers report it empty and the chain
  retires it (the client may retire it itself after the grace), and
  nothing watches those addresses any more.

**Generations are separate pools. Nothing is ever moved between them.**
This is a rule of the protocol, not an option: a rotation has
no field that asks for a sweep, no action reports one, and
the signing policy of `keel-tss` has no context that could describe a
transfer from one generation's address to another's (a consolidation may
pay only the signing key's own hot address, gas funding only the key's own
deposit index). The old pool empties through the withdrawals the chain
points at it and through nothing else. The reasons:

- *Separation of concern.* Each generation is one key, one signer, one set
  of addresses and one set of coins. What one generation holds is exactly
  what its own deposits brought minus its own payouts; the accounting of
  each key can be audited on the external chain on its own.
- *No link on the external chain, and no exposure of the new key to the
  old.* A security rotation assumes the old key, its signer or its address
  list may be compromised. A transfer from the old key to the new one would
  make the new key's addresses depend on a transaction built and signed by
  the very signer being retired, and would put the new addresses one hop
  from the old ones for anyone watching the external chain. Draining by
  payouts to users' own destinations does neither: the new key's history
  starts clean, from its own deposits.
- *Nothing to attest.* A move between two keys of the same custodian
  changes no balance on Keel, so it would be an on-chain transaction the
  observers must build, sign, watch and vote on for no ledger effect, and
  a signing policy that must know "the other generation". Without it the
  policy for custody moves is closed: every destination it may pay is a
  child of the very key that signs.

A pending rotation can be dropped with `CancelCustodyRotation`;
`GET /v1/custody/{client}` lists every live generation (`gen`, `status`,
`drain_until`, `reported_empty`) and any pending rotation. A rotation is
billed as `pools.usage_rotation_usd_micro`.

**What changes and what does not, from the exchange's side.** "Address"
means two different things here, and a rotation touches only one of them.

- *Keel addresses never change.* The exchange's own account, its revenue and
  treasury accounts, every user's account, every balance and every open
  order are Keel accounts: 32-byte Ed25519 keys on Keelchain, owned by
  whoever holds that key. They are not derived from the vault key at all,
  so a rotation does not look at them. The exchange's integration keeps
  the same account ids and the same API calls before, during and after.
- *External deposit addresses change.* What rotates is the vault key on one
  external chain. A deposit address is that key's child at the account's
  index (BIP32 non-hardened derivation on the secp256k1 chains, Keel's
  additive Ed25519 rule on Solana; `docs/deposits.md`), so the same index
  under the new key is a new string on Bitcoin, Tron, BSC or Solana. Index
  0, the exchange's own top-up and hot address, changes the same way. Every
  account keeps its index; nothing has to be re-assigned, and
  `GET /v1/custody/{client}/{chain}/addresses` returns the new strings from
  the activation block on. Users who saved an old address are not harmed
  (next point), but the exchange should show them the new one: the new
  address is the deposit address they get from now on.
- *Old and new addresses coexist while the old generation drains.* The old
  key is still watched: a deposit to an old address is credited to the same
  account as before. The new generation is the one the chain hands out for
  deposits.
- *Withdrawals are paid from whichever generation holds funds.* The chain
  points each withdrawal batch at one generation: the oldest draining one
  while it has coins, else the active one. A transaction spends one
  generation's coins only and returns change to that same key; two
  generations are never mixed in one transaction, and the observers never
  top one up from the other. From the user's side nothing changes: a
  withdrawal is paid, from an address the exchange's vault owns.
- *Old funds are never moved to the new addresses.* See above: this is the
  rule, not a setting. The old pool shrinks only as it pays withdrawals.
  If the exchange wants its coins concentrated under the new key, the way
  is an ordinary one, outside the vault's internal moves: withdraw them to
  a destination it controls and deposit them to a new-generation address,
  which the chain treats like any other withdrawal and deposit, with
  nothing linking the two keys on Keel's side. In every case balances on
  Keel are untouched: a user who held 10 USDT in custody holds 10 USDT in
  custody throughout.
- *The end of the old generation.* After `pools.grace_blocks` and once the
  observers report it empty, the chain retires it and stops watching it. A
  deposit that arrives at an old address after that is not seen; this is
  the one case where a saved old address loses coins, and the grace exists
  to make it rare. Recovery through `RequestRecovery` does not apply, since
  the retired key is no longer served.

The client rebalances its floats only when they drift, with the two paths
that already exist: `WithdrawCustody` paid to its own network deposit
address moves custody float into network float; `Withdraw` from its
`deposit` balance paid to its vault's index-0 address moves it back
(credited as the client's custody, because index 0 is its own top-up
address). Two-way user flow nets out, so external transactions shrink to the
net drift and batch like any payout. `GET /v1/custody/{client}` shows both
floats per asset. A swap is free: it is one ledger posting and sends
nothing on any network (`clients.usage_swap_usd_micro`, 0 by default).

**What Keel does**

- Observers watch the client's addresses on Bitcoin, Ethereum and Tron and
  attest deposits with the same proofs and quorum as the network vault.
- The ledger credits the right account, keeps the vault's reserve and the sum
  of its balances in balance, and halts withdrawals from that vault for an
  asset whose reserve no longer covers the balances (the halt clears on its
  own when the client covers the gap). The network vault's own reserve check
  leaves client vaults out entirely.
- `GET /v1/custody/{client}` shows the client's vaults and, per asset, the
  reserve against the balances it backs; `/v1/custody/{client}/{chain}/
  addresses` lists its addresses and their owners; the explorer renders the
  same. That page is the client's proof of reserves, and it costs the client
  nothing to publish.

**What the client never gives up**

- Its keys. No share of them lives on a Keel machine.
- Its fee schedule. Keel does not take a percentage of anything the client
  charges its users.
- Its ledger, if it wants to keep one. Two ways are supported: *rails only*
  (one chain account per client holds the aggregate; per-user balances stay
  in the client's own database) and *transparent* (one chain account per
  user, the client signs as custodian, users see their balance on the
  explorer).

**What Keel charges** (in dollars, paid in USDT, set by governance): the
plan fee of the tier the client bought (and keeps the KEEL locked for), every 30 days, and
the usage prices `clients.usage_address_usd_micro` per custody address
issued and `clients.usage_outbound_usd_micro` per custody withdrawal,
drawn from the plan's allowance first and then from the custodian's USDT
balance whoever signed the action (`docs/clients.md`, Pricing). The client sees every charge as a posting on the explorer, like any
other. Compared with a custody provider: no invoice, no per-call opacity, no
vendor that can drop a subscription or a webhook, one API for every network,
and the same numbers visible to the client's users.

</div>
<div class="tab" data-mode="noncustodial">

### Model B: the P2P client

This is for a marketplace whose users hold their own keys. Users install Keel
Wallet, the client's site asks the wallet to connect, and every action the
user takes (an offer, a trade, a release, a withdrawal) is signed by the user
and visible on the explorer. Funds sit in Keel's network vault, signed by the
observer set; escrow is a restricted account on chain; disputes are ruled by
the bonded arbitrator the offer names, normally the client's own dispute
team, or by the governance-elected set when the offer names none.

The client holds operator roles (attester for its KYC tiers, treasury for its
revenue) and scope-limited session keys so that trading does not need a popup
per click. It never holds a user's spendable key. A user may also keep
bridged coins *in their wallet*, under a Bitcoin or Tron key of their own
derived from the same seed, and move them into Keel only to trade
(`wallet.md` §5b); nothing about the client's integration changes.

**What Keel charges:** the plan fee of the client's tier every 30 days and
usage per flow (`clients.usage_fill_usd_micro`, `usage_release_usd_micro`,
`usage_ruling_usd_micro`, `usage_outbound_usd_micro`), in dollars paid in
USDT, never a share of the client's fees. While the plan runs the client's
users pay no protocol fee; the client sets its own retail fee
(`SetClientFee`), and the chain pays that fee into the client's account in
the same posting as the release. Each user account locks 1 KEEL for its
action capacity, which the client sends at onboarding and the user gets
back on unlocking. A usage charge never blocks a user's trade: what the
client cannot pay is owed and
collected from its next USDT, and the rails (addresses, withdrawals) stay
closed meanwhile. `clients.md` has the numbers.

**Where the trust is.** In Model A the client holds its own key and signer;
no share of it lives on a Keel machine. Observers only watch the client's
addresses, attest deposits (with light-client proofs on Bitcoin and
Ethereum, quorum on Tron) and build transactions that the client's own
signer checks against the chain's outbound rows before signing, from a node
the client may run itself. What the client trusts Keel for is deposit
detection, transaction construction and liveness, the same things it trusts
a custody API provider for today; the coins never move without its key.
Model B is the only model that custodies through Keel's threshold key, and
a planned option (escrow-only custody, version 0.3.0) lets a seller back an
offer from a wallet it holds on the native chain, so the vault holds coins
only while a trade is in escrow.

</div>
<div class="tab" data-mode="user">

### A wallet user, with no client

This is a person with Keel Wallet who uses the chain directly: no exchange
attested the account, no plan pays for its actions, and nobody signs for
it. The account is an ordinary Ed25519 key in the extension; the user
locks 1 KEEL for action capacity (a hundred actions a day, returned in
full three days after unlocking), asks the chain for a deposit address on
each network, and trades on the book and in escrow with everyone else,
custodial clients' users included once those have swapped in.

Funds sit where a P2P client's users' funds sit: bridged coins in Keel's
network vault, threshold-signed by the observer set, and KEEL and USDT on
the ledger. The user may also keep Bitcoin or Tron **in their wallet**,
under a key derived from the same seed, outside the chain, and move them
into Keel only to trade (`wallet.md` §5b; the *In Keel / In your wallet*
tabs on the deposits page).

**What Keel charges:** the protocol fees on value moved, 10 bps on a fill
taken, 1% on an escrow release and the flat fee on a withdrawal
(`how-it-works.md` §5, `governance.md`), and nothing per action. There is
no plan and no usage price: those are for clients. A user who later joins
an exchange is attested by it and, while the exchange's plan runs, pays no
protocol fee and needs no lock of their own.

**Where the trust is.** The user's key never leaves the extension; the
chain refuses anything that moves the balance without it. Bridged coins in
Keel are custodied by the bonded observer set under the reserve invariant
(`how-it-works.md` §9.4), not by a company; coins in the user's own wallet
are custodied by nobody but the user, and a lost phrase loses them.

</div>
</div>

## Control: can a client have signing power over the escrow?

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

A custodial client already holds the key that signs its own vault; nothing
leaves it without the client's signer. The question only arises for the
balances its users swap onto the `deposit` side to trade (the float swap
above): while there, they sit in escrow and in the network vault like any
P2P client's, and the options in the *Non-custodial client* tab apply to
them in the same way. A custodial client that wants its own team to rule
disputes on those trades bonds it as an arbitrator (option 3) and names it
on its users' offers (`clients.md`, "Naming your dispute team").

</div>
<div class="tab" data-mode="noncustodial">

A P2P client may want a say over the funds its trades put in escrow, beyond
what the protocol guarantees. The honest options, in increasing weight:

1. **Bond as an observer.** An account that posts `min_observer_bond` and is
   voted into the observer set by governance holds a real threshold share and
   co-signs every outbound of the network vault. This is full custody
   participation, with the bond at stake for misbehaviour.
2. **A client-scoped vault.** The Model A machinery pointed at Model B: a vault
   whose signer set is the client's key plus Keel observers, with a threshold
   that cannot be met without the client. Withdrawals of the client's users
   then need the client's signature.
3. **Bond as an arbitrator.** An elected arbitrator rules on disputes for
   trades it is assigned; that is control over the outcome of escrow, not over
   the keys.
4. **Hold KEEL.** Governance sets the observer set, the arbitrator set, the
   attesters and every fee. A client that holds KEEL votes on who signs.
   Whether the chain ends up governed by its clients is a question of how
   much KEEL they hold, not of code.

None of these is switched on by default; each is a governance decision.

</div>
<div class="tab" data-mode="user">

The escrow of a trade is a restricted account on chain: neither trader can
move it, and neither can any client. Your say over it is the arbitrator
you accept. An offer names the arbitrator that rules a dispute on it
(several, and the taker chooses), or names nobody and falls to the
governance-elected set; the wallet and the explorer show that next to the
price, so you take an offer with eyes open, or filter it out. Beyond that,
KEEL you hold votes on the observer set, the arbitrator set, the attesters
and every fee, the same vote a client has.

</div>
</div>

## Trading between the two models

Both models produce the same unit on the ledger (`BTC.BTC` is `BTC.BTC`,
whichever vault backs it), so a trade between them is an ordinary chain
trade once the custodial side has swapped in. There are three pairings and
one mechanism:

| Who trades with whom | Where the trade happens | Who rules a dispute |
|---|---|---|
| A custodial client's user and a Keel Wallet user (a P2P client's user or a direct user) | the chain's book or escrow | the arbitrator the offer names, else the governance set |
| A custodial client's user and another custodial client's user | the chain's book or escrow | the same |
| Two users of the same custodial client | on chain without leaving custody (`TransferCustody` through the client's escrow account), or the client's own ledger if it prefers | the client's own support, as today |

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

**The path, step by step, for a custodial user selling BTC to a wallet
user:**

1. The user (or the client on the user's behalf, `on_behalf_of`) swaps
   `SwapCustody { to: "deposit" }`: the user's custody balance becomes the
   client's, the client's network float becomes the user's `deposit`
   balance. One ledger posting, no coins move, both reserve checks hold.
2. The balance is now a `deposit` balance like any wallet user's: the user
   places an order on `BTC-USDT`, or posts a P2P offer and the escrow takes
   the coins when a taker starts a trade. The counterparty never learns
   which model the other side is in.
3. A fill or a release settles on chain with the protocol fee and, on top,
   each side's own client's retail fee: the chain pays the custodial
   client's `SetClientFee` rate into the client's account because the user
   is attested by that client, and the P2P client's rate into its account
   for its user. Keel's share is the wholesale fee only.
4. A dispute on an escrow trade goes to the arbitrator the offer named.
   The custodial client bonds its dispute team as an arbitrator
   (`Bond { role: Arbitrator }`, `min_arbitrator_bond`) and names it on
   its users' offers; a wallet user who takes the offer accepts that team,
   or filters it out. Offers that name nobody fall to the governance set.
   The custodial client's internal support never touches a chain trade.
5. The proceeds (USDT, or BTC bought) sit on the `deposit` side; the user
   or the client swaps them back with `to: "custody"` when the user is done
   trading, or leaves them to trade again. Nothing moves on Bitcoin.

**What the custodial client needs for this:** a network float in each asset
its users trade (its own `RequestDepositAddress`, funded from its vault by
`WithdrawCustody` to that address) and, if it
wants its own team ruling disputes, an arbitrator bond. It keeps its keys,
its ledger and its UI; its users see one balance and one button. A client
that would rather not run a float can let its users swap only what they
sell, with the proceeds swapped back in the same session, so the float
rarely exceeds one session's volume.

**What it costs the user:** the client's usage charge for the swap, if the
client passes it on, and the ordinary trade fees. No external chain fee at
any point, because no external transaction happens.

**What it does not do:** it does not let a custody balance trade without
the swap, and it does not let two custodial clients settle against each
other off chain. A custody balance is backed by one vault, and the chain
only settles what the network vault backs. That is what keeps each client's
reserve check exact.

</div>
<div class="tab" data-mode="noncustodial">

Nothing about your integration changes when a counterparty is a custodial
client's user. By the time such a user places an order or posts an offer,
their balance has been swapped onto the `deposit` side and is held by the
network vault like your users'; the book and the escrow do not know, and
cannot show, which side it came from. Two things are worth telling your
users:

- **A dispute may be ruled by the other client's team.** A custodial
  client bonds its dispute team as an arbitrator and names it on its
  users' offers. Your user who takes such an offer accepts that team; your
  site shows `arbitrators[]` of `GET /v1/offers` next to the price so they
  can, or filters offers down to the arbitrators you accept
  (`clients.md`, "Naming your dispute team"). Offers your users post name
  your team, so the custodial side faces the same choice.
- **Fees are per side.** A fill or a release settles with the protocol fee
  (waived for your users while your plan runs) and, on top, each side's
  own client's retail fee: yours into your account for your user, theirs
  into theirs. Neither side pays the other's.

</div>
<div class="tab" data-mode="user">

You can trade with anyone on the chain: another wallet user, a P2P
client's user, or a custodial exchange's user who has swapped a balance
onto the book. The counterparty's balance is held by the network vault
like yours by the time it trades, so the trade itself is the same. Two
things to look at before you take an offer:

- **Who rules a dispute.** An offer names its arbitrator; an exchange's
  offers name that exchange's dispute team. Taking the offer accepts that
  team. The wallet and the explorer show it; an offer that names nobody is
  ruled by the chain's elected set.
- **What you pay.** The protocol fee on the fill or the release, from your
  side only. The other side's exchange charges its own user its own retail
  fee, never you.

</div>
</div>

## The per-network setting

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

A client turns Keel on one network at a time. For a network that is on, new
deposit addresses and new withdrawals go through your vault on Keel;
everything that started before stays on the old path until it completes;
no funds are moved between the two. Before turning a network on, register
the custody vault for it (`RegisterCustodyVault`) and check
`GET /v1/ready/{chain}`: the vault epoch and signers, the age of the last
checkpoint, the last credited deposit, pending outbounds and whether the
vault is halted. `clients.md` has the checklist.

</div>
<div class="tab" data-mode="noncustodial">

A client turns Keel on one network at a time. For a network that is on, new
deposit addresses, new withdrawals and new escrow go through Keel; everything
that started before stays on the old path until it completes; no funds are
moved between the two. Before turning a network on, the client checks
`GET /v1/ready/{chain}`: the vault epoch and signers, the age of the last
checkpoint, the last credited deposit, pending outbounds and whether the
vault is halted. `clients.md` has the checklist.

</div>
<div class="tab" data-mode="user">

There is nothing to switch. The wallet's Receive card offers a network
when the chain has a vault for it, and the Deposits tab shows whether
deposits on it are being credited. If a network is halted the wallet says
so and a withdrawal on it waits; your balance is not at risk in between.

</div>
</div>
