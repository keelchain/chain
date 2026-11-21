# What governance can change

Everything on this page is a number or a set that a `ParamChange` proposal
can move. There is no administrative key: the parameter admin was removed
on 2026-11-15, so a vote is the only way (`how-it-works.md` §4), and the
explorer's Governance page is where it is proposed, voted and executed.
Nothing here is a constant in the
code any more: the modules read every cap from chain state, so a change is
a transaction with a tx id, visible in the explorer under Governance, and
takes effect from the block that includes it. What is *not* on this page
(the last section) is fixed by the protocol or chosen by each node
operator, and changing it means a software upgrade or a config file, not a
vote.

Units: KEEL amounts are in orcas, the smallest unit (1 KEEL = 1,000,000
orcas). `bps` is
basis points (100 bps = 1%). USD values are in micro-dollars
(1,000,000 = $1). Heights are blocks; times are seconds unless the key says
`_ms`.

Read the live values at `GET /v1/params` on any node; the explorer shows
them on the Governance page with the proposal that last changed each.

## What an action costs

Keel has no gas and no fee per action. An address acts on the capacity of
the KEEL it has locked, and the parameters below define that capacity.
They are tuned by vote, not by code:

| Key | Default | What it does |
|---|---|---|
| `budget.per_locked_keel_per_day` | 100 | Actions per day granted per whole KEEL locked with `LockBudget`. The first day's worth is there when the lock lands (scaled down if the unlock delay is under a day), then the pool refills linearly over 24 h. Capacity is linear in KEEL and additive across addresses, so splitting work over many addresses buys nothing. |
| `budget.unlock_delay_secs` | 259,200 (3 days) | Delay before `UnlockBudget` returns the KEEL, in full. At most one year. This is what makes capacity cost something: the KEEL behind an action cannot be moved to another key for this long. |
| `budget.free_lock_min_keel` | 1,000,000 (1 KEEL) | A `LockBudget` of at least this many orcas, which the signer holds in full, is accepted from an address with no capacity. It is the way in for a new address. 0: no lock is free, and a new address needs earned capacity first. |
| `budget.base` | 0 | Free actions every address gets, once. **0 on every network**: a grant per address is a grant per keypair, and it cannot be priced (see below). A devnet started with `keel-node --devnet` sets it to 500 so local scripts act without locking. |
| `budget.action_fee_keel` | 0 | With `budget.base` at 0 and this above 0 the chain is in *fee mode*: an address with no earned or locked capacity left pays this many orcas per action, through the fee split. At 0 such an address is refused (`BUDGET_EXHAUSTED`). |
| `budget.activation_keel` | 100,000 (0.1 KEEL) | The least KEEL an address must hold before any budget-spending action from it is accepted (`NOT_ACTIVATED` otherwise). Spendable, locked, unlocking and bonded KEEL and the refundable offer and proposal deposits all count; trade escrow does not. Observer actions, cancels and a session key revoking itself are exempt. 0 turns the floor off. |

Why there is no free grant. Until 2026-11-21 every address started with
500 free actions, and the activation floor was meant to price them: a key
had to hold 0.1 KEEL before its grant counted. But holding is not paying.
One coin could be walked from key to key, each new key spending its grant
and using its last action to pass the coin on, so the same 0.1 KEEL
bought free actions without end. A lock does not have that hole: the KEEL
that grants capacity stays where it is for the unlock delay. A user needs
1 KEEL, locks it once, has a hundred actions a day and gets the coin back
whenever they leave. What a new address needs is that first KEEL: from the
faucet on the testnet, from a deposit or a purchase, or from the client
that onboards it.

The floor stays as a second line: an address with nothing can only send
actions that are refused before they cost the chain anything. The other
budget knobs:

| Key | Default | What it does |
|---|---|---|
| `budget.per_usd_filled` | 1 | Actions earned per whole USD an address fills on a USDT-quoted pair, taker or maker. |
| `budget.cancel_bonus` | 100,000 | Cancels draw on `min(limit + cancel_bonus, 2 × limit)`, so a capped address can always unwind. |
| `budget.max_per_block` | 200 | Most actions one address may put in one block (`BLOCK_CAP`). A free lock counts. |
| `budget.price_per_action` | 1,000 | Price of one action under `BuyBudget` (adds to `earned`). |

## Limits that used to be constants

Added as governance parameters in schema 9 (2026-11-15). Each has a bound
so a vote cannot set a cap that locks the chain out of offers, sessions or
proposals altogether.

| Key | Default | Bounds | What it limits |
|---|---|---|---|
| `sessions.max_per_principal` | 16 | 1..=1,000 | Live session keys one account may hold. |
| `sessions.max_secs` | 2,592,000 (30 days) | 60..=1 year | Longest life of a session key. |
| `p2p.min_payment_window_secs` | 300 | 0..=7 days | Shortest payment window an offer may set. |
| `p2p.max_terms_chars` | 2,000 | 1..=100,000 | Longest offer terms text. |
| `p2p.max_margin_bps` | 5,000 | 0..=10,000 | Largest absolute margin an offer may quote over the reference price. |
| `disputes.max_evidence` | 32 | 1..=10,000 | Evidence items per dispute. |
| `gov.max_title_chars` | 200 | 1..=10,000 | Proposal title length. |
| `gov.max_description_chars` | 10,000 | 1..=1,000,000 | Proposal description length. |
| `transfer.max_memo_bytes` | 256 | 0..=65,535 | Longest transfer memo. |

## Block production

| Key | Default | What it does |
|---|---|---|
| `min_block_interval_ms` | 500 | Least time between blocks while actions are pending. Below the network's two quorum round trips the leader timeout fires and blocks get slower, not faster. |
| `idle_block_interval_ms` | 5,000 | Time between blocks while the mempool is empty. Must be at least the busy interval; both at most 60,000. |

## Fees and their split

| Key | Default | What it does |
|---|---|---|
| `taker_fee_bps` | 10 | Order-book taker fee, out of what the taker receives. |
| `maker_fee_bps` | 0 | Order-book maker fee. |
| `p2p_seller_fee_bps` | 100 | P2P release fee charged to the seller. |
| `p2p_small_trade_surcharge_bps` | 100 | Extra on trades under `p2p_small_trade_usd_micro`. |
| `p2p_small_trade_usd_micro` | 50,000,000 ($50) | Threshold for the surcharge. |
| `referral_share_bps` | 2,000 | Share of the seller fee paid to a referrer the offer names (not yet applied, §17 of `how-it-works.md`). |
| `withdraw_flat_fee_usd_micro` | 1,000,000 ($1) | Flat withdrawal fee, in the withdrawn asset, on top of network cost. |
| `dispute_fee_bps` | 100 | Charged to the losing side of a dispute. |
| `fee_split_treasury_bps` | 5,000 | Share of every fee to the treasury. |
| `fee_split_validators_bps` | 4,000 | Share to validator rewards. |
| `fee_split_burn_bps` | 1,000 | Share burnt. The three must sum to 10,000. |

Client pricing (`clients.*`): the plan, the usage prices and the fee waiver.
Prices are in USD micro and are paid in USDT (the stable asset), which the
epoch buyback turns into KEEL.

| Key | Default | What it does |
|---|---|---|
| `clients.protocol_fee_waived` | 1 | 1: accounts attested by a client with a running plan and nothing owed pay no protocol fee on fills, releases, rulings and withdrawals, and the client pays the usage prices below. 0: protocol fee plus a capped retail fee for everyone. |
| `clients.fee_cap_p2p_bps` | 200 | Cap on a client's retail P2P fee while the waiver is off. |
| `clients.fee_cap_taker_bps` | 50 | Cap on a client's retail taker fee while the waiver is off. |
| `clients.fee_cap_withdraw_bps` | 100 | Cap on a client's retail withdrawal fee while the waiver is off. |
| `clients.plan_period_secs` | 2,592,000 (30 days) | Length of a plan period. Between one day and one year. |
| `clients.tier1_lock_keel` | 100,000,000,000 (100,000 KEEL) | KEEL the client's operator account must have locked for tier 1. 0 turns a tier off. |
| `clients.tier1_fee_usd_micro` | 300,000,000 ($300) | What a tier 1 period costs. |
| `clients.tier1_allowance_usd_micro` | 300,000,000 ($300) | Usage a tier 1 period includes. |
| `clients.tier2_lock_keel` | 500,000,000,000 (500,000 KEEL) | Lock for tier 2. |
| `clients.tier2_fee_usd_micro` | 1,000,000,000 ($1,000) | What a tier 2 period costs. |
| `clients.tier2_allowance_usd_micro` | 1,200,000,000 ($1,200) | Usage a tier 2 period includes. |
| `clients.tier3_lock_keel` | 2,000,000,000,000 (2,000,000 KEEL) | Lock for tier 3. |
| `clients.tier3_fee_usd_micro` | 3,000,000,000 ($3,000) | What a tier 3 period costs. |
| `clients.tier3_allowance_usd_micro` | 4,000,000,000 ($4,000) | Usage a tier 3 period includes. |
| `clients.tier1_user_actions_per_day` | 10,000 | Actions a day a tier 1 plan pays for, for all the accounts the client attested together. |
| `clients.tier2_user_actions_per_day` | 50,000 | The same for tier 2. |
| `clients.tier3_user_actions_per_day` | 250,000 | The same for tier 3. |
| `clients.user_actions_per_user_per_day` | 100 | The most one attested account takes from its client's pool in a day. |
| `clients.usage_address_usd_micro` | 20,000 ($0.02) | Per deposit address issued to one of the client's accounts. |
| `clients.usage_outbound_usd_micro` | 100,000 ($0.10) | Per withdrawal queued for one of its accounts. |
| `clients.usage_swap_usd_micro` | 0 | Per float swap of a custodial client (`SwapCustody`). Free by default: a swap is one ledger posting and moves no coins. |
| `clients.usage_fill_usd_micro` | 2,000 ($0.002) | Per order of its accounts that filled. |
| `clients.usage_release_usd_micro` | 50,000 ($0.05) | Per escrow release. |
| `clients.usage_ruling_usd_micro` | 1,000,000 ($1) | Per dispute ruling. |
| `clients.buyback_max_slippage_bps` | 500 | The epoch buyback skips an asset whose best ask is further than this above the last price. |
| `clients.buyback_dust_usd_micro` | 1,000,000 | Balances below this are not swept by the buyback. |
| `clients.service_dedicated_keel_per_period` | 0 | KEEL per 30 days for a dedicated node (informational for the gateway). |

The plan fee and the usage prices are what Keel earns from a client, and
the only revenue the fee split, and so the validators, see from its flows
while the protocol fee is waived for its users. They are priced in dollars
so a client's bill does not move with the price of KEEL, and they recur:
a plan is paid every period, usage with every flow. The lock is a
different thing: it is the client's stake and its action capacity, every
tier requires its own amount of it for as long as the plan runs, and it
comes back in full. `docs/tokenomics.md` has the
reasoning and `docs/clients.md` the client's side.

A client buys a tier with `BuyPlan` and may buy any tier whose lock its
operator account meets; the tiers are expected to ascend but each key is
set on its own. A change to a tier applies to periods that start after
it: a running period keeps the fee, allowance, lock and pool it started
with. The
dollar figures above are starting values for governance to set against
what gateways and custody providers charge.

Other prices: `pools.usage_rotation_usd_micro` (0) per custody vault
rotation, billed like usage; `recovery.fee_keel` (0) per recovery request.

## Peer-to-peer trading and disputes

| Key | Default | What it does |
|---|---|---|
| `offer_deposit` | 10,000,000 (10 KEEL) | Refundable KEEL locked per live offer. |
| `default_payment_window_secs` | 1,800 | Time a buyer has to mark paid before the seller may cancel. |
| `release_grace_secs` | 3,600 | Time after "paid" before the buyer may open a dispute. |
| `ruling_window_secs` | 86,400 | Time an arbitrator has to rule before another may (not yet applied). |
| `offer_allowance_base` | 2 | Live offers a new address may hold. |
| `offer_allowance_trades_per_step` | 10 | Completed trades per allowance step. |
| `offer_allowance_step` | 2 | Offers added per step. |
| `arbitration.max_named` | 8 | Most arbitrators one offer may name. |

## Markets and the pooled house maker

| Key | Default | What it does |
|---|---|---|
| `house_quote_max_ttl_blocks` | 60 | Longest a house quote stays valid. |
| `max_open_orders_per_pair` | 500 | Open orders per owner (today counted across pairs). |
| `liquidity.protocol_share_bps` | 5,000 | Share of the pool's gain above its high-water mark that goes to the fee split. |
| `liquidity.min_deposit_usd_micro` | 10,000,000 ($10) | Smallest deposit into the pool. |
| `liquidity.pool_cap_usd_micro` | 0 (none) | Largest pool value after a deposit. |
| `liquidity.withdraw_delay_blocks` | 10,000 | Blocks between a withdrawal and its release; at most 1,000,000. |
| `liquidity.max_quote_inventory_bps` | 10,000 | Most of the inventory one house quote side may offer. |
| `liquidity.min_spread_bps` | 0 | Smallest spread a two-sided house quote may have. |
| `liquidity.max_epoch_fee_bps` | 500 | Most one mark may take, as a share of pool value. |

## Staking, validators and observers

| Key | Default | What it does |
|---|---|---|
| `min_validator_bond` | 100,000 KEEL | Least bond to be a validator. |
| `min_observer_bond` | 500,000 KEEL | Least bond to be an observer. |
| `min_arbitrator_bond` | 50,000 KEEL | Least bond to be an arbitrator. |
| `unbonding_blocks` | 100,000 | Blocks between unbond and withdrawal. |
| `max_validators` | 100 | Size of the validator set. |
| `epoch_length_blocks` | 10,000 | Validator-set and vault epoch length. |
| `observer_reward_bps` | 2,500 | Share of validator rewards paid to observers before stake weighting. |
| `slash_double_sign_bps` | 500 | Slash for a double sign (routine exists, not yet called). |
| `slash_false_observation_bps` | 10,000 | Slash for a false observation (same). |

## The dollar: conversion and rebalancing

| Key | Default | Bounds | What it does |
|---|---|---|---|
| `stable.auto_convert` | 1 | 0 or 1 | 1: a credited USDT deposit on any network becomes the unified `USDT` balance at once; 0: deposits stay per-network and users mint by hand. |
| `stable.rebalance_max_premium_bps` | 100 (1%) | 0..=10,000 | Premium a `WithdrawStable` pays when the chosen network's reserve share is zero, scaled linearly down to nothing at its target share; the same rate is the rebate a USDT deposit on that network earns from the pool. 0 = off. |

Basket membership and the per-network caps are proposals
(`SetStableBasket { asset, cap, enabled }`), not parameters. The target
share of each network is its cap's share of the enabled caps; `GET
/v1/stable` shows reserve, cap, target, share and the live premium per
network.

## Reference prices

| Key | Default | Bounds | What it does |
|---|---|---|---|
| `prices.twap_window_secs` | 600 (10 minutes) | 0..=7 days | Window of the time-weighted average fill price per pair that settlement reads wherever it needs a dollar value: the small-trade surcharge, the withdrawal flat fee, the budget earned per dollar, the liquidity pool's mark, the buyback. The book trades at the last fill; the reference is what the chain prices with, so one print cannot set it. 0 = off (the last fill is used). |

## Governance itself

| Key | Default | What it does |
|---|---|---|
| `proposal_deposit` | 1,000,000 KEEL | Locked by `Propose`; burnt if the proposal is vetoed, refunded otherwise. |
| `voting_period_blocks` | 20,000 | Voting window. |
| `timelock_blocks` | 5,000 | Delay between a pass and `ExecuteProposal`. |
| `quorum_bps` | 3,340 | Least turnout, as a share of all bonded KEEL. |
| `threshold_bps` | 5,000 | Least yes, as a share of yes plus no. |
| `veto_bps` | 3,340 | Veto share of the votes cast that kills a proposal. |

Governance can also move its own parameters, so a vote can make the next
vote easier or harder; the deposit, quorum and veto rules apply to that
vote as to any other.

## Vaults, deposits and withdrawals

| Key | Default | What it does |
|---|---|---|
| `observer_quorum_bps` | 6,667 | Observer attestations needed, in bps of observer bond (today the governance-set count is used instead). |
| `confirmations_btc` | 2 | Bitcoin confirmations before a deposit credits. |
| `confirmations_eth` | 12 | Ethereum and BSC confirmations. |
| `confirmations_tron` | 19 | Tron confirmations; Solana observers report finalized only. |
| `deposit_daily_cap_usd_micro` | 0 (none) | Per-chain daily credit cap (not yet enforced). |
| `large_deposit_usd_micro` | 50,000,000,000 ($50,000) | Deposits above this become spendable only after the delay below. |
| `large_deposit_delay_blocks` | 600 | That delay. |
| `outbound_batch_interval_blocks` | 20 | Blocks between outbound batches per chain. |
| `pools.grace_blocks` | 1,000 | Least blocks a draining custody generation is kept before it may be retired. |
| `recovery.expiry_blocks` | 20,000 | Blocks an open recovery row waits for the observers before it expires. |

## Lightning

| Key | Default | What it does |
|---|---|---|
| `lightning_enabled` | 1 | Lightning deposits and withdrawals accepted. |
| `lightning_pool_cap_sats` | 50,000,000 | Most BTC one observer may hold in its Lightning pool. |
| `lightning_max_deposit_sats` | 10,000,000 | Largest single Lightning deposit. |
| `lightning_max_withdraw_sats` | 10,000,000 | Largest single Lightning withdrawal. |
| `lightning_max_fee_bps` | 50 | Routing fee allowance charged to the withdrawer, as bps of the amount. |
| `lightning_min_fee_sats` | 10 | Floor of that allowance. |
| `lightning_payout_timeout_blocks` | 600 | Blocks the assigned observer has to report a payout before it is refunded. |
| `lightning_daily_cap_sats` | 0 (none) | Daily Lightning deposit cap per observer. |

## Sets, listings and the other proposal kinds

Beyond numbers, a proposal can: list or delist a trading pair; register an
asset; spend from the treasury; schedule a software upgrade (version and
the height at which nodes not running it halt); set the arbitrator,
attester or observer sets, with the observer threshold; set a stablecoin
basket entry and its cap; pause a module until a height; bind a token
contract to an asset; set the Bitcoin or Ethereum light-client checkpoint;
ban an arbitrator; or carry text only.

## What governance cannot change

Fixed by the protocol (a change is a software upgrade; before mainnet the
networks simply restart from genesis, from mainnet on it is gated on an
activation height so old blocks replay unchanged):

- The ledger rules: no action freezes, seizes or edits a balance; a vault
  pays nothing without an observer quorum; the reserve invariant halts an
  asset whose reserves fall below user liabilities.
- What the chain mints and when: the block reward (10% of supply, 262.5
  million KEEL in the first year halving every four years, a tenth of each
  block to its proposer) and the community reward (25% of supply, 10 KEEL
  per dollar of protocol fee to the payer). Both are constants in
  `modules::rewards`, deliberately out of governance's reach so a
  validator's expected income does not depend on who holds the votes.
- Block capacity, `MAX_ACTIONS_PER_BLOCK` = 50,000 actions, and the
  consensus timings derived from the block intervals.
- The chain id, the address and signature scheme, the derivation of
  deposit addresses from a vault key, the snapshot schema.
- The internal scale of the pooled house maker's share price
  (`VPS_SCALE`) and the KEEL unit.

Chosen by each node operator, in its config, not by vote:

- Mempool bounds: how far a signer's nonce may run ahead (64), how long an
  unincluded action is kept (10 minutes), how many actions a proposer puts
  in one block (5,000), how many blocks of receipts a node keeps in memory.
- Which external-chain nodes an observer talks to, confirmation sources,
  signer URLs, fee fallbacks, and the recovery network list.
- Faucet amounts and rate limits on the testnet.
