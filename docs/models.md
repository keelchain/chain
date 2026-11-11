# Two ways to run an exchange on Keel

Keel is infrastructure. An exchange, a marketplace, a payments desk or a bot
is a client of the chain, and every client chooses one of two models per
network (Bitcoin, Ethereum, Tron). Both models use the same API, the same
explorer, the same SDK and the same readiness endpoints; they differ in one
question: **who holds the funds and who signs the withdrawals.**

| | Model A: custodial client | Model B: P2P client |
|---|---|---|
| Who holds user funds | The client, in **its own vault**: keys it owns, a signer it runs | Keel's network vault for that chain, threshold-signed by the observer set; escrow on chain |
| What Keel does | Detects deposits to the client's addresses, keeps the ledger, builds and batches withdrawals, estimates fees, broadcasts, records everything on the explorer | Everything in A, plus on-chain escrow, disputes, the order book and session keys |
| Who signs withdrawals | The client's own signer | The observer set (2-of-3 on the testnet) |
| Who signs user actions | The client, as custodian | The user, with Keel Wallet |
| What users can verify | Proof of reserves for that client: vault addresses, reserve against the sum of balances, every movement with a transaction id | Their own account and every action they signed |
| Client fees and margin | **Untouched.** Keel charges usage, never a share of the client's fees | **Untouched** (0.2.0). The chain waives its protocol fee for the client's users; the client sets its own retail fee and receives it in the same posting, and pays usage in KEEL per fill, release, ruling and withdrawal |
| How Keel is paid | In KEEL, bought on the chain's own order book | In KEEL, the same way |

A client can run both models at once: custodial accounts in its own vault,
wallet users on the network vault. It can also switch a network from one
model to the other with its own per-network setting; old balances stay where
they are, new deposits follow the new setting, and nothing has to be moved.

## Model A: the custodial client

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

### Rotating to a fresh pool

A custodial client that is labelled, sanctioned or rebranding can move its
whole platform to a fresh vault on a chain without touching a single Keel
account: `RotateCustodyVault { chain, public_key, chain_code, signer_url,
sweep, activate_at }`. The client makes the new key on a signer of its own
(a second `keel-tss serve`, or its KMS behind a second `POST /sign` URL;
the old signer keeps serving the old key), and registers it. From the
block it activates, or at `activate_at`:

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

`sweep` decides whether the old coins move: on, the observers sweep the
old generation to the new key's own address (attested on chain as
`PoolSwept`, the network fee charged to the client's gas like a payout);
off, nothing ever links the two pools on the external chain and the old
one simply pays withdrawals until it is dry, which is the right choice
when the old addresses are the problem. A pending rotation can be dropped
with `CancelCustodyRotation`; `GET /v1/custody/{client}` lists every live
generation (`gen`, `status`, `sweep`, `drain_until`, `swept`) and any
pending rotation. A rotation is billed as `pools.usage_rotation_keel`.

The client rebalances its floats only when they drift, with the two paths
that already exist: `WithdrawCustody` paid to its own network deposit
address moves custody float into network float; `Withdraw` from its
`deposit` balance paid to its vault's index-0 address moves it back
(credited as the client's custody, because index 0 is its own top-up
address). Two-way user flow nets out, so external transactions shrink to the
net drift and batch like any payout. `GET /v1/custody/{client}` shows both
floats per asset. A swap is billed as rails usage like a payout
(`clients.usage_outbound_keel`).

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

**What Keel charges** (all in KEEL, set by governance): the usage prices
`clients.usage_address_keel` per custody address issued and
`clients.usage_outbound_keel` per custody withdrawal, charged to the
custodian's KEEL balance whoever signed the action, plus a service tier
chosen by locking KEEL. The client sees every charge as a posting on the explorer, like any
other. Compared with a custody provider: no invoice, no per-call opacity, no
vendor that can drop a subscription or a webhook, one API for every network,
and the same numbers visible to the client's users.

## Model B: the P2P client

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

**What Keel charges:** usage in KEEL, per flow (`clients.usage_fill_keel`,
`usage_release_keel`, `usage_ruling_keel`, `usage_outbound_keel`), never a
share of the client's fees. The client's users pay no protocol fee; the
client sets its own retail fee (`SetClientFee`), and the chain pays that
fee into the client's account in the same posting as the release. A usage
charge never blocks a user's trade: what the client cannot pay is owed and
collected from its next KEEL, and the rails (addresses, withdrawals) stay
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

## Control: can a client have signing power over the escrow?

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

## Trading between the two models

Both models produce the same unit on the ledger (`BTC.BTC` is `BTC.BTC`,
whichever vault backs it), so a trade between them is an ordinary chain
trade once the custodial side has swapped in. There are three pairings and
one mechanism:

| Who trades with whom | Where the trade happens | Who rules a dispute |
|---|---|---|
| A custodial client's user and a Keel Wallet user (a P2P client's user or a direct user) | the chain's book or escrow | the arbitrator the offer names, else the governance set |
| A custodial client's user and another custodial client's user | the chain's book or escrow | the same |
| Two users of the same custodial client | the client's own ledger, never the chain | the client's own support, as today |

**The path, step by step, for a custodial user selling BTC to a wallet
user:**

1. The user (or the client on the user's behalf, `on_behalf_of`) swaps
   `SwapCustody { to: "deposit" }`: the user's custody balance becomes the
   client's, the client's network float becomes the user's `deposit`
   balance. One ledger posting, no coins move, both reserve checks hold.
2. The balance is now a `deposit` balance like any wallet user's: the user
   places an order on `BTC-KUSD`, or posts a P2P offer and the escrow takes
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
5. The proceeds (KUSD, or BTC bought) sit on the `deposit` side; the user
   or the client swaps them back with `to: "custody"` when the user is done
   trading, or leaves them to trade again. Nothing moves on Bitcoin.

**What the custodial client needs for this:** a network float in each asset
its users trade (its own `RequestDepositAddress`, funded from its vault by
`WithdrawCustody` to that address), KEEL for the swap usage, and, if it
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

## The per-network setting

A client turns Keel on one network at a time. For a network that is on, new
deposit addresses, new withdrawals and new escrow go through Keel; everything
that started before stays on the old path until it completes; no funds are
moved between the two. Before turning a network on, the client checks
`GET /v1/ready/{chain}`: the vault epoch and signers, the age of the last
checkpoint, the last credited deposit, pending outbounds and whether the
vault is halted. `clients.md` has the checklist.
