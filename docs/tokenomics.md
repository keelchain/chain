# KEEL tokenomics

## Supply

- Hard cap: **21,000,000,000 KEEL**. The smallest unit is the **orca**:
  1 KEEL = 1,000,000 orcas, so every KEEL amount on the chain, in the
  RPC and in the SDK is a count of orcas.
- 65% is issued at genesis (treasury, team, liquidity). The other 35% does
  not exist until the chain mints it: 10% as the **block reward** to
  validators and 25% as the **community reward** to the users who pay
  fees. Both are capped, so total supply converges to the cap and never
  passes it; both schedules are constants in `modules::rewards`, changed
  only by a software upgrade validators choose to run, never by a vote.
- Burn: the `fee_split_burn_bps` share of every fee (10%) plus vetoed
  proposal deposits and, once wired, slashed bonds.

## Genesis allocation (defaults in code: `keel_vm::genesis::Allocation`)

| Bucket | Share | Where it sits | How it moves |
|---|---|---|---|
| DAO treasury | 40% (the former 5% reserve folded in) | system `treasury`, issued at genesis | `TreasurySpend` proposals; also receives half of every fee |
| Community rewards | 25% | not issued; a cap in `state.rewards` | minted to a user for every protocol fee it pays, `COMMUNITY_KEEL_PER_USD` (10 KEEL) per dollar of fee, until the cap is reached. The KEEL awards already promised by the platform are paid at genesis, line by line, out of this share |
| Team and early contributors | 15% | `vesting` (granted) + `team_reserve` (ungranted), issued at genesis | 1-year cliff, then linear over 4 years, released every block; ungranted remainder by proposal |
| Validators | 10% | not issued; a cap in `state.rewards` | the block reward: 262.5M KEEL in the first year, halving every four years by time, a tenth of each block's mint to its proposer and the rest to the epoch payout |
| Liquidity (house maker + stablecoin) | 10% | system `swap_pool`, issued at genesis | the house maker's inventory from block one |

Shares are basis points and must sum to 10,000; `total_supply` defaults
to the cap. `keel genesis-from-export --allocation alloc.json` overrides
them; `--no-allocation` skips the genesis issuance on test networks, which
still mint the two rewards up to the default caps.

### Open before mainnet

| Item | What it needs |
|---|---|
| Team grants | The list of chain addresses with each one's share of the team bucket in basis points, and the cliff and duration if different from one year and four years. Until then the whole 15% waits in `team_reserve`. |

### The economics, in one paragraph

The project funds itself by selling KEEL (the team allocation as it vests,
and treasury KEEL by proposal) and earns a recurring revenue in the fees:
half of every protocol fee reaches the treasury in the asset it was paid
in, clients pay a plan every 30 days and a dollar price per flow for
their users' waived fees, and the liquidity pool pays half its gain. The
epoch buyback turns that revenue into KEEL. What holds the price while
KEEL is sold is what the chain makes people hold or buy: KEEL locked for
action capacity by every account that acts on its own (linear in volume),
the lock a client's plan tier requires, bonds, and the buyback of the plan and
usage income. Against those stand the
sellers: the treasury and vesting, validators selling block rewards to
cover costs, and community reward recipients. That is why issuance is kept
modest and halving, and why community rewards are earned on fees rather
than dripped on a calendar.

## Demand sinks

- Validator bond (min 100,000 KEEL), observer bond (500,000), arbitrator
  bond (50,000); slashed on misconduct.
- Governance vote weight.
- Action capacity by locking (the Tron energy model): each whole KEEL
  locked grants `budget.per_locked_keel_per_day` actions per day (default
  100), and there is no free base (`budget.base` is 0), so every account
  that acts on its own holds a lock: one KEEL for a retail user, in
  proportion to its volume for anyone larger, however many addresses it
  spreads it over. The users of a client with a plan act on the plan's
  pool instead.
  Unlocking returns the KEEL in full after `budget.unlock_delay_secs`
  (default 3 days). Nothing is paid, so it adds demand without adding a
  fee. Buying budget (0.001 KEEL per action) stays as a fallback.
- The plan tier lock: a client's operator account keeps 100,000, 500,000
  or 2,000,000 KEEL locked for tier 1, 2 or 3, on top of the fee.
  Refundable, and held for as long as the plan runs.
- The buyback: plan fees and usage are paid in USDT and bought into KEEL
  on the chain's own book every epoch, in proportion to what clients use.
- Offer listing deposit: 10 KEEL per live offer (refundable).
- Proposal deposit: 1,000,000 KEEL (refunded on pass or reject; burned on veto).
- Fee tier discounts for stakers (planned parameter).
- Liquidity-pool deposits: KEEL, USDT or BTC put into the
  house maker's pool earns the depositor's share of the spread; the
  protocol keeps `liquidity.protocol_share_bps` (50%) of the pool's gain
  through the fee split.

## Fee flows (defaults in `keel_vm::Params`, editable live)

What you pay Keel depends on which of three you are, so pick your tab:
the fees below are the whole list for that path, and the other two do not
apply to you. Every number is a chain parameter, and only a `ParamChange`
proposal moves it: there is no administrative key (`docs/governance.md`
lists every key).

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

**Keel takes no share of your fees.** You hold your users' coins in your
own vault and charge them whatever you like on your own ledger; the chain
never sees those fees. You pay a membership and a price per use of the
rails. Your users pay Keel nothing.

Two different things are charged on the rails, and they are not the same
money. The **service price** is Keel's: fixed, in dollars, the same on
every network. The **network fee** is the external chain's: it changes
with that network, Keel adds nothing to it and keeps none of it.

| What you pay Keel | Price | When |
|---|---|---|
| KEEL kept locked | 100,000 / 500,000 / 2,000,000 by tier, returned in full on unlock | while the plan runs |
| Plan fee | $300 / $1,000 / $3,000 by tier, in USDT; includes $300 / $1,200 / $4,000 of the service prices below | every 30 days |
| Service price: deposit address issued in your vault | $0.02, on any network | per address |
| Service price: withdrawal from your vault, or a float swap | $0.10, on any network | per withdrawal or swap |
| Service price: vault rotation | $0 (`pools.usage_rotation_usd_micro`) | per rotation |
| A user's balance you swapped onto the book: filled order / escrow release / dispute ruling | $0.002 / $0.05 / $1 | per flow |

How it is taken: from the USDT on your operator account on Keel, your
plan's included usage first, inside the action that asks for the address
or queues the withdrawal (`UsageCovered`, `UsageCharged`). If the account
cannot cover it, that action is refused and nothing is taken.

| What you pay the network | Price | When |
|---|---|---|
| Network fee of a withdrawal from your vault | what the transaction actually cost on that network at that moment (table below the tabs) | when the payout confirms |
| Network fee of a deposit | nothing through Keel: the sender pays it on the external chain | never |

How it is taken: your vault pays the fee, because it is your vault that
sends. The chain then takes exactly that amount from your **gas**: the
native coin of that network (BTC, TRX, BNB, SOL) you keep as a custody
balance at your vault's own address. No estimate and no margin. If the gas
is empty the payout still goes out and the fee is booked against you,
settled from your next gas top-up.

| What does not apply to you | |
|---|---|
| Taker fee, P2P release fee, dispute fee, withdrawal flat fee | waived for your users while your plan runs |
| A share of your trading, withdrawal or any other fee | never |
| A fee per action | none; your users act on your plan's daily pool |

</div>
<div class="tab" data-mode="noncustodial">

**Keel takes no share of your fees.** Your users hold their own keys in
Keel Wallet and trade on the chain. While your plan runs they pay none of
the protocol fees; you set your own fee (`SetClientFee`, uncapped) and the
chain collects it from your user and pays it to you in the same block,
in full. You pay a membership and a small fixed price per flow.

| What you pay | Price | When |
|---|---|---|
| KEEL kept locked | 100,000 / 500,000 / 2,000,000 by tier, returned in full on unlock | while the plan runs |
| Plan fee | $300 / $1,000 / $3,000 by tier, in USDT; includes $300 / $1,200 / $4,000 of the usage below | every 30 days |
| Order of one of your users that filled | $0.002 | per order |
| Escrow release (a completed P2P trade) | $0.05 | per release |
| Dispute ruling | $1 | per ruling |
| Deposit address issued to one of your users | $0.02 | per address |
| Withdrawal by one of your users | $0.10 | per withdrawal |

| What your users pay | |
|---|---|
| To you | the fee you set: a share of a P2P release, of a taker fill, of a withdrawal |
| To Keel | nothing on trades, releases or rulings |
| To the network | on a withdrawal, that network's fee at that moment (table below the tabs): an estimate is locked when the withdrawal is queued, in the network's native coin if the user holds it and otherwise in the coin being withdrawn; the actual fee is settled when the payout confirms and the difference is refunded |
| To act | nothing, and no KEEL: their actions come out of your plan's daily pool |

| What does not apply to you | |
|---|---|
| Taker fee (10 bps), P2P release fee (1%), dispute fee (1%), withdrawal flat fee ($1) | waived for your users while your plan runs and nothing is owed |
| A share of your fees | never |

</div>
<div class="tab" data-mode="user">

**You pay a share of the value you move, and nothing to act.** You came
with a wallet and no exchange. There is no plan and no usage price for
you; these are the only fees.

| Fee | Rate | Who pays |
|---|---|---|
| Order book, taking an order | 10 bps | the taker, from what it receives |
| Order book, making an order | 0 | nobody |
| P2P trade released | 1% (+1% under $50) | the seller |
| Withdrawal, to Keel | $1 flat | the withdrawer |
| Withdrawal, to the network | that network's fee at that moment (table below the tabs): an estimate is locked when you withdraw, in the network's native coin if you hold it and otherwise in the coin you withdraw; the actual fee is settled when the payout confirms and the difference comes back to you | the withdrawer |
| Dispute | 1% of the escrow | the side that loses |
| Acting at all | no fee: 1 KEEL locked gives a hundred actions a day and comes back on unlock | you lock, you do not pay |

If an exchange with a plan onboards your account, its tab applies instead:
these fees stop and you pay what that exchange charges.

</div>
</div>

**Network fees move, per network.** They are not a Keel parameter and no
number here would stay true: the observers report each network's current
rate to the chain (`ReportNetworkFee`), the chain uses the median, and the
fee actually paid is what is charged. The live rate is `fee_rate` in `GET
/v1/vaults/{chain}`.

| Network | The rate the observers report | What a payout is sized at |
|---|---|---|
| Bitcoin | satoshis per virtual byte | about 200 vB |
| Ethereum, BNB Smart Chain | gas price | 21,000 gas for the native coin, 65,000 for a token |
| Tron | the cost of one transfer, in sun | one transfer |
| Solana | lamports per signature | one signature for SOL; up to 410 for a token payout that has to create the recipient's token account |

Two things are the same on every path. The liquidity pool pays 50% of its
gain above its high-water mark, in kind, each epoch (it is the pool's
depositors who pay that, not traders). And everything Keel collects, on
any path, is split the same way: 50% treasury, 40% validators, observers
and delegators (observers take 25% of that share first), 10% burn. All
splits are on-chain ledger legs, auditable per fee.

## Block rewards and epoch payouts

Every block mints KEEL for the time since the previous block, at the
block-reward rate of its era. A tenth goes straight to the block's
proposer; the rest lands in `validator_rewards` next to the fees. At each
epoch boundary that pool is paid out, per asset and in kind (USDT, BTC,
KEEL): observers first take `observer_reward_bps` split equally, then
validators pro rata to bonded power, each delegator receiving its slice
automatically. KEEL has no price model, so the block reward is defined as
coins per time, not a value: what it is worth follows the demand sinks
above. `GET /v1/supply` shows the rate, the next halving, and what remains
of both caps.

## Two kinds of user, two ways of paying

A person who comes with a wallet and a business that brings its users pay
differently; which usage a business pays depends on which model it runs
(`models.md`).

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

**A custodial client pays a plan and rails usage.** The membership below is
the same for every client: the tier's lock and the plan fee every 30 days.
The usage it incurs is the rails: a price per deposit address issued in its
own vault (`clients.usage_address_usd_micro`), per withdrawal its signer
pays out and per float swap (`clients.usage_outbound_usd_micro`), and per
vault rotation (`pools.usage_rotation_usd_micro`). Its users sign nothing
on the chain unless it lets them. Capacity is charged to whoever signs the
envelope: an action the client signs from its operator account
(`on_behalf_of` a user) spends the operator lock's capacity, and an action
signed with a per-user key the client holds spends that account's share of
the plan's daily pool, so a transparent-ledger user needs no KEEL either. Users whose balance it swaps onto the book
incur the fill, release and ruling usage like a non-custodial client's.

</div>
<div class="tab" data-mode="noncustodial">

**A non-custodial client pays a plan and flow usage.** The membership below
is the same for every client: the tier's lock and the plan fee every 30
days. The usage it incurs is per flow of its users: a filled order
(`clients.usage_fill_usd_micro`), an escrow release
(`clients.usage_release_usd_micro`), a dispute ruling
(`clients.usage_ruling_usd_micro`), a deposit address and a withdrawal
(`clients.usage_address_usd_micro`, `clients.usage_outbound_usd_micro`).
Its users sign their own actions in Keel Wallet, so the plan's daily pool
of actions is what spares each of them a lock of their own.

</div>
<div class="tab" data-mode="user">

**A user pays nothing to act and locks one KEEL.** Someone who arrives
with a wallet to post an offer, place a limit order or move KEEL needs a
handful of actions. They lock 1 KEEL, which is a hundred actions a day,
and get it back in full three days after they unlock it. That lock is the
whole spam defence: an attacker's capacity is the KEEL they tie up, per
day, whatever the number of keys. What a user does pay is a share of the
value they move (the fee table in their tab above), not a price per transaction. There
is no plan and no usage for a user; the rest of this section is about
clients.

</div>
</div>

**An exchange pays a plan.** A gateway and a ledger are what an exchange
rents elsewhere for hundreds or thousands of dollars a month, and a lock
alone would be a one-time, refundable purchase: it takes KEEL off the
market once and pays the validators and the treasury nothing. So a client
pays a membership, which is a lock and a fee together, and usage. Three
parts, of which only the first is a deposit:

| Part | What it is | Recurs |
|---|---|---|
| Operator lock | KEEL locked on the client's operator account, as much as its tier requires: its action capacity and its standing in the plan. Returned in full on unlock | no |
| Plan fee | a dollar price per 30-day period, paid in USDT, which includes a usage allowance and a daily pool of actions for the client's users | every period |
| Usage | a dollar price per deposit address, withdrawal, filled order, escrow release and ruling, drawn from the allowance first and from the client's USDT after | with every flow |

| Tier | Lock | Plan fee per 30 days | Usage included | Actions a day for its users |
|---|---|---|---|---|
| 1 | 100,000 KEEL | $300 | $300 | 10,000 |
| 2 | 500,000 KEEL | $1,000 | $1,200 | 50,000 |
| 3 | 2,000,000 KEEL | $3,000 | $4,000 | 250,000 |

These are the defaults in code and every one is a `ParamChange` key
(`clients.tier<N>_lock_keel`, `_fee_usd_micro`, `_allowance_usd_micro`,
`_user_actions_per_day`, `clients.plan_period_secs`, `clients.usage_*_usd_micro`). They are
starting values to be set against what gateways and custody providers
charge, not a price list I consider final.

Why dollars and USDT. A price fixed in KEEL makes the service dearer when
KEEL rises and the revenue smaller when it falls, and an exchange budgets
in dollars. Paying in USDT needs no price oracle for KEEL: the chain
collects the stable asset through the ordinary fee split and the buyback
(below) converts it on its own order book, so the income is demand for
KEEL in proportion to use, every epoch. The exchange holds KEEL for its
lock and USDT for its bill.

Why an exchange does not skip the plan. Without one it is still an
attester, but its users are not waived: each of them pays the protocol
fees (10 bps on a fill, 1% on a release, $1 on a withdrawal), which at any
real volume is more than the plan. The plan is the cheaper way to be a
client, and it is the only way to keep all of one's own fees.

Why both a lock and a fee. The fee is the income: it recurs, it scales
with the tier, and it is what the validators and the treasury are paid
from. The lock is the commitment: KEEL held off the market for as long as
the client is a client, in proportion to its size, which a fee alone
would not ask for. A client that unlocks below its tier has its plan
suspended at once.

Why the plan carries its users. A lock per user is the right price for a
person who came on their own, and the wrong chore for an exchange with a
hundred thousand accounts: it would have to send each one a KEEL and
manage it. So each tier includes a daily pool of actions for the accounts
the client attested, capped per user. That is not a free grant to anyone
with a keypair, which is what made the old free budget a sybil hole: the
pool is finite, it is paid for, and only accounts a registered client
vouched for draw on it.

Who enforces it. The plan is part of the state machine, not a billing
service beside it. The tiers, each client's period, its allowance, its
users' pool and its debt are chain state; the purchase and the renewal are
executed in a block like any transfer, and a usage price is charged inside
the action that incurs it, so every validator applies the same charges by
executing the same blocks. No validator meters usage or decides anything,
and no key can waive a charge.

Where it is bought. Keel Harbor ([harbor.keelchain.com](https://harbor.keelchain.com/)) is the client's
account: plans, payment, usage against the allowance, the lock, retail
fees and vaults. It is a static page with no server behind it. A client
deposits USDT to its operator account, locks the tier's KEEL, and signs
`BuyPlan`; Harbor only builds the action and shows what the chain
answers, so there is no second ledger to reconcile and nothing Harbor
could charge that the chain would not.

How a period runs. `BuyPlan { tier }` from an account whose lock meets the
tier takes the fee and starts the period in that block. The client holds
the allowance until the period ends, when the plan renews in the same
block at the tier it set (`SetPlanRenewal`; 0 stops it). If it cannot pay
then, it has no plan and is renewed in the first block in which it can; a
plan fee is never carried as debt. A higher tier can be bought before the
period ends, with the unused part of the running period taken off the
fee. Usage beyond the allowance comes out of the client's USDT; the
per-flow charges never block a user's trade, so what the client cannot pay
then is owed, and while anything is owed no new address or withdrawal is
issued to its accounts, its users pay the protocol fees again and its pool
pays for none of their actions. `GET /v1/clients/{address}` shows the
tier, the period, the allowance left, what was paid and what is owed.

## Wholesale and retail: how clients and Keel are paid

Every payment to Keel ends as a purchase of KEEL; nothing is invoiced.

**Protocol fees** (governance parameters) are collected in the asset of the
flow: `p2p_seller_fee_bps` on an escrow release, `taker_fee_bps` on a fill,
`withdraw_flat_fee_usd_micro` plus the network fee on a withdrawal,
`dispute_fee_bps` on a ruling, and the budget purchases. Each fee is split
at collection into the `treasury`, `validator_rewards` and `burn` system
accounts (`fee_split_*_bps`).

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

**Clients keep all their fees.** A custodial client charges its users
whatever its own ledger says; the chain never sees those fees and takes no
share of them. On the chain the client is an attester, so when one of its
users swaps a balance onto the book and trades, the protocol fee is waived
while the plan runs and the client's `SetClientFee` schedule (`p2p_bps`,
`taker_bps`, `withdraw_bps`) is charged to that user and credited to the
client's deposit account in the same block, up to what the payer holds.

**The plan and usage** are what the client pays Keel, through the same fee
split: the plan fee when a period starts, then
`clients.usage_address_usd_micro` per custody address issued,
`clients.usage_outbound_usd_micro` per custody withdrawal or float swap,
and `pools.usage_rotation_usd_micro` per vault rotation. Fill, release and
ruling usage applies to its users' trades on the book after a swap.

</div>
<div class="tab" data-mode="noncustodial">

**Clients keep all their fees.** A client is an attester: the account that
vouched for a user's tier. An account attested by a client with a running
plan pays no protocol fee on fills, escrow releases, dispute rulings or
the withdrawal flat fee. The client publishes its own schedule with
`SetClientFee` (`p2p_bps`, `taker_bps`, `withdraw_bps`, uncapped while the
waiver is on), and the chain charges that retail fee to the client's user
in the same block and credits it to the client's deposit account, up to
what the payer holds.

**The plan and usage** are what a client pays Keel, through the same fee
split: the plan fee when a period starts, then `clients.usage_address_usd_micro`
per deposit address issued, `clients.usage_outbound_usd_micro` per
withdrawal queued, `clients.usage_fill_usd_micro` per order of its users
that filled, `clients.usage_release_usd_micro` per escrow release and
`clients.usage_ruling_usd_micro` per dispute ruling. `clients.protocol_fee_waived
= 0` switches the waiver off for everyone (protocol fee plus a capped
retail fee).

</div>
<div class="tab" data-mode="user">

**A user pays the protocol fees.** A user with no attester, or whose
attester has no running plan, pays the protocol fees above on what they
move, and nobody charges them a retail fee. If an exchange attests the
account later, the waiver and that exchange's `SetClientFee` schedule
apply from then on, while its plan runs.

</div>
</div>

**The buyback.** At every epoch boundary the chain sweeps the non-KEEL
balances of the three system accounts: for each asset with a `KEEL-<asset>`
pair (`KEEL-USDT` at launch) it places a market buy for KEEL with that
balance against the order book, moves the KEEL bought back into the same
system account and returns any budget the book could not fill. The burn
account's KEEL is the burn. Balances below `clients.buyback_dust_usd_micro`
are left alone, and an asset whose best ask sits more than
`clients.buyback_max_slippage_bps` above the last price is skipped that
epoch. Validators are therefore paid in KEEL, the treasury holds KEEL, and
fee income is demand for KEEL on the chain's own market. `GET
/rpc/v1/treasury` shows the balances and the last buyback per asset.

**A dedicated node** is paid by a KEEL transfer to the treasury
(`clients.service_dedicated_keel_per_period` per 30 days).
