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

Every number below is a chain parameter, and only a `ParamChange`
proposal moves it: there is no administrative key (`docs/governance.md`
lists every key).

| Fee | Rate | Payer |
|---|---|---|
| Book taker | 10 bps | taker, from what it receives |
| Book maker | 0 | — |
| P2P release | 1% (+1% under $50) | seller |
| Withdrawal | network cost + $1 flat | withdrawer |
| Dispute | 1% of escrow | losing side |
| Liquidity pool profit share | 50% of the pool's gain above its high-water mark, in kind, each epoch | the pool (depositors) |

Accounts attested by a client with a running plan pay none of the first
four; their client pays the plan and usage instead (below), in dollars
settled in USDT: $0.002 per filled order, $0.05 per release, $0.10 per
withdrawal, $0.02 per deposit address, $1 per ruling by default (all
governance parameters).

Split: 50% treasury, 40% validators + observers + delegators (observers
take 25% of that share first), 10% burn. All splits are on-chain ledger
legs, auditable per fee.

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

**A user pays nothing to act and locks one KEEL.** Someone who arrives
with a wallet to post an offer, place a limit order or move KEEL needs a
handful of actions. They lock 1 KEEL, which is a hundred actions a day,
and get it back in full three days after they unlock it. That lock is the
whole spam defence: an attacker's capacity is the KEEL they tie up, per
day, whatever the number of keys. What a user does pay is a share of the
value they move (the fee table above), not a price per transaction.

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

Where it is bought. Keel Harbor (`harbor.keelchain.com`) is the client's
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

**Clients keep all their fees.** A client is an attester: the account that
vouched for a user's tier. An account attested by a client with a running
plan pays no protocol fee on fills, escrow releases, dispute rulings or
the withdrawal flat fee. The client publishes its own schedule with
`SetClientFee` (`p2p_bps`, `taker_bps`, `withdraw_bps`, uncapped while the
waiver is on), and the chain charges that retail fee to the client's user
in the same block and credits it to the client's deposit account, up to
what the payer holds. A user with no attester, or whose attester has no
plan, pays the protocol fees.

**The plan and usage** are what a client pays Keel, through the same fee
split: the plan fee when a period starts, then `clients.usage_address_usd_micro`
per deposit address issued, `clients.usage_outbound_usd_micro` per
withdrawal queued, `clients.usage_fill_usd_micro` per order of its users
that filled, `clients.usage_release_usd_micro` per escrow release and
`clients.usage_ruling_usd_micro` per dispute ruling. `clients.protocol_fee_waived
= 0` switches the waiver off for everyone (protocol fee plus a capped
retail fee).

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
