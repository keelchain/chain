# KEEL tokenomics (draft v0.1, open decisions marked TBD)

## Supply

- Hard cap: **21,000,000,000 KEEL** (matches the existing `KEEL_MAX_SUPPLY`).
- Smallest unit: 10^-6 KEEL.
- No inflation beyond the cap. Operators are paid from fee revenue (in kind,
  through the fee split) and, since 2026-11-15, from the genesis validator
  bucket paid out over time (`emission.validator_bps_per_year`, 10% of what
  remains per year by default): coins that exist from genesis, reaching
  validators, observers and delegators pro rata to stake.
- Burn: the `fee_split_burn_bps` share of every fee (launch proposal 10%)
  plus vetoed proposal deposits and slashed bonds.

## Genesis allocation (defaults in code: `keel_vm::genesis::Allocation`)

The whole 21B is issued at genesis into DAO-controlled buckets. Nothing
leaves a bucket without a governance proposal, except team grants, which
vest on a schedule the chain enforces.

| Bucket | Share | Where it sits | How it moves |
|---|---|---|---|
| DAO treasury | 35% | system `treasury` | `TreasurySpend` proposals |
| Community rewards (trading, liquidity, referrals) | 25% | system `community_pool` | proposals; the KEEL awards already promised by the platform are paid from this bucket at genesis, line by line |
| Team and early contributors | 15% | `vesting` (granted) + `team_reserve` (ungranted) | 1-year cliff, then linear over 4 years, released every block; ungranted remainder by proposal |
| Validator / observer bootstrap | 10% | system `validator_bootstrap` | proposals (bonds for the first operator sets) |
| Liquidity (house maker + stablecoin) | 10% | system `swap_pool` | the house maker's inventory from block one |
| Strategic reserve | 5% | system `strategic_reserve` | proposals |

Shares are basis points and must sum to 10,000; `total_supply` defaults
to the cap. `keel genesis-from-export --allocation alloc.json` overrides
them; `--no-allocation` skips issuance on test networks.

### What I still have to decide, and what each needs

| Decision | Needed | Default until decided |
|---|---|---|
| Team grants | A list of (chain address, share of the team bucket in bps) and, if different, the cliff and duration | no grants; the whole 15% waits in `team_reserve` |
| Bucket shares | Only if the 35/25/15/10/10/5 split changes; must sum to 100% | the split above |
| Community emissions | A schedule (how much per epoch to trading/referral rewards); executed as proposals or a later emissions module | nothing emitted automatically |
| Validator bootstrap use | Decided 2026-11-15: the bucket is the staking emission, `emission.validator_bps_per_year` of what remains per year, accrued at each epoch boundary into `validator_rewards` and distributed pro rata (`docs/governance.md`) | 10% a year |
| Super admin | The chain address that will hold `param_admin` (the backoffice super admin's chain key) | devnet: seed 0; mainnet: must be passed to `genesis-from-export --param-admin` |

## Demand sinks

- Validator bond (min 100,000 KEEL), observer bond (500,000), arbitrator
  bond (50,000); slashed on misconduct.
- Governance vote weight.
- Action capacity by locking (2026-09-08, the Tron energy model):
  each whole KEEL locked grants `budget.per_locked_keel_per_day` actions per
  day (default 100) on top of a free base that is only an onboarding grant
  (`budget.base`, 500 actions since 2026-11-15; a per-address grant is a
  per-keypair grant, so volume cannot be free); unlocking returns the KEEL
  in full after `budget.unlock_delay_secs` (default 3 days). Nothing is
  paid, so it adds demand without adding a fee, and the demand is linear
  in use: an exchange locks in proportion to its volume however many
  addresses it spreads it over. Buying budget (0.001 KEEL per action)
  stays as the no-wait fallback. An address also needs
  `budget.activation_keel` (0.1 KEEL) before any action counts.
- Offer listing deposit: 10 KEEL per live offer (refundable).
- Proposal deposit: 1,000,000 KEEL (refunded on pass or reject; burned on veto).
- Fee tier discounts for stakers (planned parameter).
- Liquidity-pool deposits (2026-10-02): KEEL, KUSD or BTC put into the
  house maker's pool earns the depositor's share of the spread; the
  protocol keeps `liquidity.protocol_share_bps` (50%) of the pool's gain
  through the fee split.

## Fee flows (defaults in `keel_vm::Params`, editable live)

Every number below is a chain parameter. At launch the platform super
admin changes them directly from the backoffice (Fees page → "Chain
parameters", signed with `CHAIN_ADMIN_SECRET`, the `SetParam` action);
governance can take that right away with a `SetParamAdmin { admin: None }`
proposal, after which only `ParamChange` proposals can move them.

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
ruling by default (all governance parameters; zero before 2026-11-15,
which made the waiver a full subsidy).

Split: 50% treasury, 40% validators + observers + delegators (observers
take 25% of that share first), 10% burn. All splits are on-chain ledger
legs, auditable per fee.

## Epoch rewards

At each epoch boundary the accumulated `validator_rewards` balance per asset
is paid out in kind (KUSD, BTC.BTC, …), so operators earn the exchange's
revenue directly. The same boundary first accrues the staking emission:
`remaining × emission.validator_bps_per_year × elapsed / year` of the
`validator_bootstrap` bucket moves into `validator_rewards` in KEEL and is
paid by the same distribution (observers' share first, then validators and
their delegators pro rata to power). KEEL has no price model, so the
emission is defined as a share of stake, not a value: what it is worth
follows the demand sinks above, chiefly KEEL locked for action capacity and
KEEL clients buy to pay usage.

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
pair (`KEEL-KUSD` at launch) it places a market buy for KEEL with that
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
