# KEEL tokenomics (draft v0.1, open decisions marked TBD)

## Supply

- Hard cap: **21,000,000,000 KEEL** (matches the existing `KEEL_MAX_SUPPLY`).
- Smallest unit: 10^-6 KEEL.
- No inflation beyond the cap. Fee revenue, not issuance, pays operators.
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
| Validator bootstrap use | Whether the first operators get their bonds from this bucket (and the lock-up) | nothing distributed |
| Super admin | The chain address that will hold `param_admin` (the backoffice super admin's chain key) | devnet: seed 0; mainnet: must be passed to `genesis-from-export --param-admin` |

## Demand sinks

- Validator bond (min 100,000 KEEL), observer bond (500,000), arbitrator
  bond (50,000); slashed on misconduct.
- Governance vote weight.
- Action capacity by locking (2026-09-08, the Tron energy model):
  each whole KEEL locked grants `budget.per_locked_keel_per_day` actions per
  day (default 100) on top of the free base; unlocking returns the KEEL in
  full after `budget.unlock_delay_secs` (default 3 days). Nothing is paid,
  so it adds demand without adding a fee. Buying budget (0.001 KEEL per
  action) stays as the no-wait fallback.
- Offer listing deposit: 10 KEEL per live offer (refundable).
- Proposal deposit: 1,000,000 KEEL (refunded on pass or reject; burned on veto).
- Fee tier discounts for stakers (planned parameter).

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

Split: 50% treasury, 40% validators + observers + delegators (observers
take 25% of that share first), 10% burn. All splits are on-chain ledger
legs, auditable per fee.

## Epoch rewards

At each epoch boundary the accumulated `validator_rewards` balance per asset
is paid out in kind (KUSD, BTC.BTC, …), so operators earn the exchange's
revenue directly rather than a native inflation subsidy.
