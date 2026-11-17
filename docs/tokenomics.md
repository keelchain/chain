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
in, clients pay KEEL per flow for their users' waived fees, and the
liquidity pool pays half its gain. The epoch buyback turns that revenue
into KEEL. What holds the price while KEEL is sold is what the chain makes
people hold or buy: KEEL locked for action capacity (linear in volume),
bonds, the activation dust, and the usage prices. Against those stand the
sellers: the treasury and vesting, validators selling block rewards to
cover costs, and community reward recipients. That is why issuance is kept
modest and halving, and why community rewards are earned on fees rather
than dripped on a calendar.

## Demand sinks

- Validator bond (min 100,000 KEEL), observer bond (500,000), arbitrator
  bond (50,000); slashed on misconduct.
- Governance vote weight.
- Action capacity by locking (the Tron energy model):
  each whole KEEL locked grants `budget.per_locked_keel_per_day` actions per
  day (default 100) on top of a free base that is only an onboarding grant
  (`budget.base`, 500 actions; a per-address grant is a per-keypair grant,
  so volume cannot be free); unlocking returns the KEEL
  in full after `budget.unlock_delay_secs` (default 3 days). Nothing is
  paid, so it adds demand without adding a fee, and the demand is linear
  in use: an exchange locks in proportion to its volume however many
  addresses it spreads it over. Buying budget (0.001 KEEL per action)
  stays as the no-wait fallback. An address also needs
  `budget.activation_keel` (0.1 KEEL) before any action counts.
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

Accounts attested by a live client pay none of the first four; their
client pays usage in KEEL instead (below): 0.01 KEEL per filled order,
0.1 KEEL per release, per withdrawal and per deposit address, 1 KEEL per
ruling by default (all governance parameters).

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

## Wholesale and retail: how clients and Keel are paid

Every payment to Keel is a purchase of KEEL; nothing is invoiced in fiat.

**Protocol fees** (governance parameters) are collected in the asset of the
flow: `p2p_seller_fee_bps` on an escrow release, `taker_fee_bps` on a fill,
`withdraw_flat_fee_usd_micro` plus the network fee on a withdrawal,
`dispute_fee_bps` on a ruling, and the budget purchases. Each fee is split
at collection into the `treasury`, `validator_rewards` and `burn` system
accounts (`fee_split_*_bps`).

**Clients keep all their fees (0.2.0).** A client is an attester: the
account that vouched for a user's tier. An account attested by a live
client pays no protocol fee on fills, escrow releases, dispute rulings or
the withdrawal flat fee. The client publishes its own schedule with
`SetClientFee` (`p2p_bps`, `taker_bps`, `withdraw_bps`, uncapped while the
waiver is on), and the chain charges that retail fee to the client's user
in the same block and credits it to the client's deposit account, up to
what the payer holds. `GET /rpc/v1/clients/{address}` shows a client's
schedule, what it has earned per asset, its usage paid and anything it
owes. A user with no attester pays the protocol fees and no retail fee.

**Usage** is what a client pays Keel for the rails, in KEEL through the
protocol fee split: `clients.usage_address_keel` per deposit address
issued, `clients.usage_outbound_keel` per withdrawal queued,
`clients.usage_fill_keel` per order of its users that filled,
`clients.usage_release_keel` per escrow release and
`clients.usage_ruling_keel` per dispute ruling. The three per-flow charges
never block the user's trade: what the client cannot pay is owed, collected
from its next KEEL (at the epoch boundary or on its next charge), and no
new address or withdrawal is issued to its accounts while it is owed. All
five prices are zero until governance sets them; every one is a
`ParamChange` key. `clients.protocol_fee_waived = 0` restores the earlier
pricing (protocol fee plus a capped retail fee).

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

**Service tiers** are read from the KEEL a client's operator account has
locked (`LockBudget`), and a dedicated node is paid by a KEEL transfer to
the treasury (`clients.service_dedicated_keel_per_period` per 30 days).
