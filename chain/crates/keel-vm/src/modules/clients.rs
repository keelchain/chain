//! Clients as fee recipients and payers.
//!
//! A client is an attester: the account that vouched for a user's KYC tier
//! (`Attest`). Two things follow from that link, both settled on chain:
//!
//! - **Retail fees.** The client sets its own fee schedule (`SetClientFee`,
//!   basis points on P2P releases, order-book fills and withdrawals, each
//!   capped by governance). When one of its attested accounts pays a
//!   protocol fee, the retail fee is charged to the same payer in the same
//!   block and credited to the client's deposit account. The retail leg is
//!   taken from what the payer's deposit account holds, up to the amount
//!   due, so a release or a fill is never blocked by it.
//! - **Usage.** A client whose account requests a deposit address or queues
//!   a withdrawal pays the governance-set usage price. Prices are in
//!   dollars (`usage_*_usd_micro`) and settled in the stable asset through
//!   the ordinary fee split, which the epoch buyback turns into KEEL. A
//!   client that cannot pay has no new addresses or withdrawals issued to
//!   its users; that is its bill.
//!
//! - **The plan (2026-11-21).** A client's bill has a floor: a membership
//!   fee plus a lock. The client buys a tier (`BuyPlan`): its operator
//!   account must have that tier's KEEL locked (`LockBudget`), the tier's
//!   fee is taken in the stable asset, and the period (`plan_period_secs`,
//!   30 days) starts in that block. The fee includes a usage allowance,
//!   which the usage prices draw on before the client's balance, and a
//!   daily pool of actions for the accounts the client attested, so its
//!   users need no KEEL of their own. When the period ends the plan
//!   renews by itself at the tier the client chose (`SetPlanRenewal`), in
//!   the first block in which the client can pay. A plan counts only
//!   while the lock it was bought with stays in place.
//!
//! - **Waived protocol fees.** An account attested by a client with a
//!   running plan and nothing owed pays no protocol fee on fills, escrow
//!   releases, dispute rulings and the withdrawal flat fee; the client
//!   pays a price per flow instead (`usage_fill_usd_micro`,
//!   `usage_release_usd_micro`, `usage_ruling_usd_micro`, and the
//!   per-outbound price). Those charges never block the user: what the
//!   client cannot pay is owed and collected from its next stable balance,
//!   and meanwhile the rails (addresses, withdrawals) stay closed and its
//!   users pay the protocol fees again. With the fee waived the retail
//!   caps no longer apply. A client without a plan is an attester like
//!   before, but its users pay the protocol fees.
//!
//! Governance knobs live here as `clients.*` parameters (`ParamChange` and
//! `ParamChange` route them), so `Params` keeps its layout.

use crate::{
    modules::{fees, tokens},
    receipt::{Event, VmError},
    state::State,
};
use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::ClientFee;
use keel_ledger::{Record, TxType};
use keel_types::{mul_div_floor, Address, Amount, Asset, ORCAS_PER_KEEL};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One plan tier: the lock that selects it, what a period costs and the
/// usage that fee includes.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct PlanTier {
    /// Orcas the client's operator account must have locked. 0 = the tier
    /// is off.
    pub lock_keel: u128,
    /// USD micro charged at the start of every period.
    pub fee_usd_micro: u128,
    /// USD micro of usage the period includes.
    pub allowance_usd_micro: u128,
    /// Actions per day the client's attested accounts may take from the
    /// plan, all of them together.
    pub user_actions_per_day: u64,
}

/// How many tiers a network has (`clients.tier1_*` .. `clients.tier3_*`).
pub const TIERS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ClientParams {
    /// Caps on a client's retail schedule, in basis points.
    pub fee_cap_p2p_bps: u32,
    pub fee_cap_taker_bps: u32,
    pub fee_cap_withdraw_bps: u32,
    /// USD micro a client pays per deposit address issued to one of its
    /// accounts, and per withdrawal queued. 0 = free.
    pub usage_address_usd_micro: u128,
    pub usage_outbound_usd_micro: u128,
    /// The epoch buyback skips an asset whose best ask is more than this
    /// above the last price.
    pub buyback_max_slippage_bps: u32,
    /// Balances below this USD value are not swept.
    pub buyback_dust_usd_micro: u64,
    /// KEEL per 30-day period for a dedicated node (informational for the
    /// gateway; paid by transfer to the treasury).
    pub service_dedicated_keel_per_period: u128,
    /// 1: accounts attested by a client with a running plan pay no
    /// protocol fee on fills, releases, rulings and withdrawals; the
    /// client pays the per-flow usage prices below instead and its retail
    /// schedule is uncapped. 0: protocol fee plus capped retail for all.
    pub protocol_fee_waived: u32,
    /// USD micro a client pays per order that filled, per escrow release
    /// and per dispute ruling of its accounts. 0 = free.
    pub usage_fill_usd_micro: u128,
    pub usage_release_usd_micro: u128,
    pub usage_ruling_usd_micro: u128,
    /// Length of a plan period, in seconds.
    pub plan_period_secs: u64,
    /// The plan tiers, lowest first. A client is in the highest tier whose
    /// lock its operator account meets.
    pub tiers: [PlanTier; TIERS],
    /// Actions per day one attested account may take from its client's
    /// plan, so one user cannot use up the pool.
    pub user_actions_per_user_per_day: u64,
}

impl Default for ClientParams {
    fn default() -> Self {
        Self {
            fee_cap_p2p_bps: 200,
            fee_cap_taker_bps: 50,
            fee_cap_withdraw_bps: 100,
            // What a client pays Keel for the rails and for the flows whose
            // protocol fee its users no longer pay: the only revenue the
            // fee split (and so the validators) see from client flows.
            // Dollars, so the bill does not move with the price of KEEL.
            usage_address_usd_micro: 20_000, // $0.02 per deposit address issued
            usage_outbound_usd_micro: 100_000, // $0.10 per withdrawal queued
            buyback_max_slippage_bps: 500,
            buyback_dust_usd_micro: 1_000_000,
            service_dedicated_keel_per_period: 0,
            protocol_fee_waived: 1,
            usage_fill_usd_micro: 2_000, // $0.002 per order that filled
            usage_release_usd_micro: 50_000, // $0.05 per escrow release
            usage_ruling_usd_micro: 1_000_000, // $1 per dispute ruling
            plan_period_secs: 30 * 86_400,
            tiers: [
                PlanTier {
                    lock_keel: 100_000 * ORCAS_PER_KEEL,
                    fee_usd_micro: 300_000_000,
                    allowance_usd_micro: 300_000_000,
                    user_actions_per_day: 10_000,
                },
                PlanTier {
                    lock_keel: 500_000 * ORCAS_PER_KEEL,
                    fee_usd_micro: 1_000_000_000,
                    allowance_usd_micro: 1_200_000_000,
                    user_actions_per_day: 50_000,
                },
                PlanTier {
                    lock_keel: 2_000_000 * ORCAS_PER_KEEL,
                    fee_usd_micro: 3_000_000_000,
                    allowance_usd_micro: 4_000_000_000,
                    user_actions_per_day: 250_000,
                },
            ],
            user_actions_per_user_per_day: 100,
        }
    }
}

/// A client's running plan period.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
pub struct Plan {
    /// 1-based tier the period was bought at.
    pub tier: u8,
    /// Block time (seconds) the period started and ends at.
    pub started_secs: u64,
    pub ends_secs: u64,
    /// USD micro the tier cost when the period started.
    pub fee_usd_micro: u128,
    /// USD micro of usage the period included, and what is left of it.
    pub allowance_usd_micro: u128,
    pub allowance_left_usd_micro: u128,
    /// Orcas the tier required locked when the period started. The plan
    /// counts only while the client's lock is at least this.
    pub lock_keel: u128,
    /// The tier the plan renews at when the period ends; 0 = it does not.
    pub renew_tier: u8,
    /// Actions per day the client's users may take from the plan, and how
    /// many they took on `user_actions_day` (block time in whole days).
    pub user_actions_per_day: u64,
    pub user_actions_used: u64,
    pub user_actions_day: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ClientsState {
    pub params: ClientParams,
    /// Subject -> the attester that last vouched for it.
    pub attested_by: BTreeMap<Address, Address>,
    /// Client -> its retail schedule.
    pub fees: BTreeMap<Address, ClientFee>,
    /// (client, asset) -> retail fees earned so far.
    pub earned: BTreeMap<(Address, Asset), Amount>,
    /// (client, asset) -> usage and plan fees paid so far.
    pub usage_paid: BTreeMap<(Address, Asset), Amount>,
    /// Asset -> (height, spent, KEEL bought) of the last buyback.
    pub last_buyback: BTreeMap<Asset, (u64, Amount, Amount)>,
    /// Client -> usage it could not pay when it fell due, in the stable
    /// asset. Collected from the client's next stable balance; while it is
    /// owed, no new addresses or withdrawals are issued to the client's
    /// accounts and its users' protocol fees are not waived.
    pub usage_owed: BTreeMap<Address, Amount>,
    /// Trade id -> the client billed at release because the seller's
    /// protocol fee was waived at trade start.
    pub waived_trades: BTreeMap<u64, Address>,
    /// Client -> its plan period (kept after it ends, until the next one
    /// replaces it).
    pub plans: BTreeMap<Address, Plan>,
}

pub const PARAM_KEYS: &[&str] = &[
    "clients.fee_cap_p2p_bps",
    "clients.fee_cap_taker_bps",
    "clients.fee_cap_withdraw_bps",
    "clients.usage_address_usd_micro",
    "clients.usage_outbound_usd_micro",
    "clients.buyback_max_slippage_bps",
    "clients.buyback_dust_usd_micro",
    "clients.service_dedicated_keel_per_period",
    "clients.protocol_fee_waived",
    "clients.usage_fill_usd_micro",
    "clients.usage_release_usd_micro",
    "clients.usage_ruling_usd_micro",
    "clients.plan_period_secs",
    "clients.tier1_lock_keel",
    "clients.tier1_fee_usd_micro",
    "clients.tier1_allowance_usd_micro",
    "clients.tier2_lock_keel",
    "clients.tier2_fee_usd_micro",
    "clients.tier2_allowance_usd_micro",
    "clients.tier3_lock_keel",
    "clients.tier3_fee_usd_micro",
    "clients.tier3_allowance_usd_micro",
    "clients.tier1_user_actions_per_day",
    "clients.tier2_user_actions_per_day",
    "clients.tier3_user_actions_per_day",
    "clients.user_actions_per_user_per_day",
];

/// `clients.tier<N>_<field>` -> (index, field).
fn tier_key(key: &str) -> Option<(usize, &str)> {
    let rest = key.strip_prefix("clients.tier")?;
    let (n, field) = rest.split_once('_')?;
    let n: usize = n.parse().ok()?;
    (1..=TIERS).contains(&n).then_some((n - 1, field))
}

pub fn get_param(state: &State, key: &str) -> Option<u128> {
    let p = &state.clients.params;
    if let Some((i, field)) = tier_key(key) {
        let t = &p.tiers[i];
        return match field {
            "lock_keel" => Some(t.lock_keel),
            "fee_usd_micro" => Some(t.fee_usd_micro),
            "allowance_usd_micro" => Some(t.allowance_usd_micro),
            "user_actions_per_day" => Some(t.user_actions_per_day as u128),
            _ => None,
        };
    }
    Some(match key {
        "clients.fee_cap_p2p_bps" => p.fee_cap_p2p_bps as u128,
        "clients.fee_cap_taker_bps" => p.fee_cap_taker_bps as u128,
        "clients.fee_cap_withdraw_bps" => p.fee_cap_withdraw_bps as u128,
        "clients.usage_address_usd_micro" => p.usage_address_usd_micro,
        "clients.usage_outbound_usd_micro" => p.usage_outbound_usd_micro,
        "clients.buyback_max_slippage_bps" => p.buyback_max_slippage_bps as u128,
        "clients.buyback_dust_usd_micro" => p.buyback_dust_usd_micro as u128,
        "clients.service_dedicated_keel_per_period" => p.service_dedicated_keel_per_period,
        "clients.protocol_fee_waived" => p.protocol_fee_waived as u128,
        "clients.usage_fill_usd_micro" => p.usage_fill_usd_micro,
        "clients.usage_release_usd_micro" => p.usage_release_usd_micro,
        "clients.usage_ruling_usd_micro" => p.usage_ruling_usd_micro,
        "clients.plan_period_secs" => p.plan_period_secs as u128,
        "clients.user_actions_per_user_per_day" => p.user_actions_per_user_per_day as u128,
        _ => return None,
    })
}

/// Shortest and longest plan period governance may set: a day, a year.
const MIN_PLAN_PERIOD_SECS: u64 = 86_400;
const MAX_PLAN_PERIOD_SECS: u64 = 365 * 86_400;

/// `ParamChange` for the `clients.*` keys.
pub fn set_param(state: &mut State, key: &str, value: u128) -> Result<Vec<Event>, VmError> {
    let bps = |v: u128| -> Result<u32, VmError> {
        u32::try_from(v)
            .ok()
            .filter(|b| *b <= 10_000)
            .ok_or_else(|| VmError::Invalid(format!("{key} must be within 0..=10000 bps")))
    };
    let p = &mut state.clients.params;
    if let Some((i, field)) = tier_key(key) {
        let t = &mut p.tiers[i];
        match field {
            "lock_keel" => t.lock_keel = value,
            "fee_usd_micro" => t.fee_usd_micro = value,
            "allowance_usd_micro" => t.allowance_usd_micro = value,
            "user_actions_per_day" => {
                t.user_actions_per_day =
                    u64::try_from(value).map_err(|_| VmError::Invalid("too large".into()))?
            }
            _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
        }
        return Ok(vec![Event::ParamChanged {
            key: key.to_string(),
            value,
        }]);
    }
    match key {
        "clients.fee_cap_p2p_bps" => p.fee_cap_p2p_bps = bps(value)?,
        "clients.fee_cap_taker_bps" => p.fee_cap_taker_bps = bps(value)?,
        "clients.fee_cap_withdraw_bps" => p.fee_cap_withdraw_bps = bps(value)?,
        "clients.usage_address_usd_micro" => p.usage_address_usd_micro = value,
        "clients.usage_outbound_usd_micro" => p.usage_outbound_usd_micro = value,
        "clients.buyback_max_slippage_bps" => p.buyback_max_slippage_bps = bps(value)?,
        "clients.buyback_dust_usd_micro" => {
            p.buyback_dust_usd_micro =
                u64::try_from(value).map_err(|_| VmError::Invalid("too large".into()))?
        }
        "clients.service_dedicated_keel_per_period" => p.service_dedicated_keel_per_period = value,
        "clients.protocol_fee_waived" => {
            if value > 1 {
                return Err(VmError::Invalid(format!("{key} must be 0 or 1")));
            }
            p.protocol_fee_waived = value as u32;
        }
        "clients.usage_fill_usd_micro" => p.usage_fill_usd_micro = value,
        "clients.usage_release_usd_micro" => p.usage_release_usd_micro = value,
        "clients.usage_ruling_usd_micro" => p.usage_ruling_usd_micro = value,
        "clients.plan_period_secs" => {
            p.plan_period_secs = u64::try_from(value)
                .ok()
                .filter(|v| (MIN_PLAN_PERIOD_SECS..=MAX_PLAN_PERIOD_SECS).contains(v))
                .ok_or_else(|| {
                    VmError::Invalid(format!("{key} must be within one day..=one year"))
                })?
        }
        "clients.user_actions_per_user_per_day" => {
            p.user_actions_per_user_per_day =
                u64::try_from(value).map_err(|_| VmError::Invalid("too large".into()))?
        }
        _ => return Err(VmError::Invalid(format!("cannot set {key}"))),
    }
    Ok(vec![Event::ParamChanged {
        key: key.to_string(),
        value,
    }])
}

/// The 1-based tier `locked` orcas select, 0 when below every tier.
pub fn tier_for_lock(p: &ClientParams, locked: u128) -> u8 {
    p.tiers
        .iter()
        .enumerate()
        .filter(|(_, t)| t.lock_keel > 0 && locked >= t.lock_keel)
        .map(|(i, _)| i as u8 + 1)
        .max()
        .unwrap_or(0)
}

fn locked_keel(state: &State, client: &Address) -> u128 {
    state
        .account_ref(client)
        .map(|m| m.budget.locked)
        .unwrap_or(0)
}

/// `client`'s plan while its period runs and the lock it was bought with
/// is in place. Unlocking below the tier suspends the plan; locking again
/// brings back what is left of the period.
pub fn active_plan<'a>(state: &'a State, client: &Address, now_secs: u64) -> Option<&'a Plan> {
    state
        .clients
        .plans
        .get(client)
        .filter(|p| now_secs < p.ends_secs && locked_keel(state, client) >= p.lock_keel)
}

/// What is left of a running period when the client upgrades: the share
/// of its fee for the time not used, and no more than the share of the
/// allowance not used.
fn upgrade_credit(plan: &Plan, now_secs: u64) -> u128 {
    let length = u128::from(plan.ends_secs.saturating_sub(plan.started_secs));
    let left = u128::from(plan.ends_secs.saturating_sub(now_secs));
    if length == 0 {
        return 0;
    }
    let by_time = mul_div_floor(plan.fee_usd_micro, left, length).unwrap_or(0);
    let by_usage = if plan.allowance_usd_micro == 0 {
        by_time
    } else {
        mul_div_floor(
            plan.fee_usd_micro,
            plan.allowance_left_usd_micro,
            plan.allowance_usd_micro,
        )
        .unwrap_or(0)
    };
    by_time.min(by_usage)
}

/// Take `fee` (and first whatever the client owes) from the client's
/// stable balance and start a period at `tier` now. The caller has checked
/// the tier and the lock. Nothing is taken when the balance does not cover
/// both.
fn start_plan(
    state: &mut State,
    client: Address,
    tier: u8,
    fee: u128,
    now_secs: u64,
    external_id: &str,
) -> Result<Vec<Event>, VmError> {
    let stable = state.tokens.stable.clone();
    let from = tokens::deposit_key(client, &stable);
    let owed = state.clients.usage_owed.get(&client).copied().unwrap_or(0);
    let have = state.ledger.balance(&from).max(0) as Amount;
    let need = owed.saturating_add(fee);
    if have < need {
        return Err(VmError::Invalid(format!(
            "tier {tier} costs {need} {stable} micro now ({fee} fee, {owed} owed); the account holds {have}"
        )));
    }
    let mut events = Vec::new();
    if owed > 0 {
        fees::collect(
            state,
            &format!("{external_id}:owed"),
            TxType::BudgetPurchase,
            None,
            from.clone(),
            &stable,
            owed,
        )?;
        state.clients.usage_owed.remove(&client);
        note_paid(state, client, &stable, owed);
        events.push(Event::UsageCharged {
            client,
            asset: stable.clone(),
            amount: owed,
            kind: "owed".into(),
        });
    }
    if fee > 0 {
        fees::collect(
            state,
            external_id,
            TxType::BudgetPurchase,
            None,
            from,
            &stable,
            fee,
        )?;
        note_paid(state, client, &stable, fee);
    }
    let t = state.clients.params.tiers[tier as usize - 1].clone();
    let ends_secs = now_secs.saturating_add(state.clients.params.plan_period_secs);
    state.clients.plans.insert(
        client,
        Plan {
            tier,
            started_secs: now_secs,
            ends_secs,
            fee_usd_micro: t.fee_usd_micro,
            allowance_usd_micro: t.allowance_usd_micro,
            allowance_left_usd_micro: t.allowance_usd_micro,
            lock_keel: t.lock_keel,
            renew_tier: tier,
            user_actions_per_day: t.user_actions_per_day,
            user_actions_used: 0,
            user_actions_day: now_secs / 86_400,
        },
    );
    events.push(Event::ClientPlanStarted {
        client,
        tier,
        asset: stable,
        fee,
        allowance: t.allowance_usd_micro,
        ends_at: ends_secs,
    });
    Ok(events)
}

fn check_tier(p: &ClientParams, tier: u8) -> Result<&PlanTier, VmError> {
    (1..=TIERS as u8)
        .contains(&tier)
        .then(|| &p.tiers[tier as usize - 1])
        .filter(|t| t.lock_keel > 0)
        .ok_or_else(|| VmError::Invalid(format!("there is no plan tier {tier}")))
}

/// `BuyPlan`: an attester buys a period at `tier`, starting now. With a
/// period running, only a higher tier can be bought: the upgrade starts a
/// fresh period and what the old one had left is taken off its fee.
pub fn apply_buy_plan(
    state: &mut State,
    ctx: &crate::context::BlockContext,
    signer: Address,
    tx_id: &[u8; 32],
    tier: u8,
) -> Result<Vec<Event>, VmError> {
    if !state.attest.attesters.contains(&signer) {
        return Err(VmError::Unauthorized);
    }
    let t = check_tier(&state.clients.params, tier)?.clone();
    let locked = locked_keel(state, &signer);
    if locked < t.lock_keel {
        return Err(VmError::Invalid(format!(
            "tier {tier} needs {} orcas locked; {locked} are locked",
            t.lock_keel
        )));
    }
    let now = ctx.seconds();
    let mut fee = t.fee_usd_micro;
    if let Some(plan) = state
        .clients
        .plans
        .get(&signer)
        .filter(|p| now < p.ends_secs)
    {
        if tier <= plan.tier {
            return Err(VmError::Invalid(format!(
                "a tier {} plan runs until {}; only a higher tier can be bought before then",
                plan.tier, plan.ends_secs
            )));
        }
        fee = fee.saturating_sub(upgrade_credit(plan, now));
    }
    start_plan(
        state,
        signer,
        tier,
        fee,
        now,
        &format!("plan:{}", tokens::hex(tx_id)),
    )
}

/// `SetPlanRenewal`: the tier the signer's plan renews at; 0 stops it.
pub fn apply_set_renewal(
    state: &mut State,
    signer: Address,
    tier: u8,
) -> Result<Vec<Event>, VmError> {
    if tier != 0 {
        check_tier(&state.clients.params, tier)?;
    }
    let plan = state
        .clients
        .plans
        .get_mut(&signer)
        .ok_or_else(|| VmError::Invalid("no plan to renew; buy one first".into()))?;
    plan.renew_tier = tier;
    Ok(vec![Event::ClientPlanRenewalSet {
        client: signer,
        tier,
    }])
}

/// The client whose plan pays for `user`'s next action, if any: `user` has
/// a live attestation from a client that is not itself, that client's plan
/// is running with nothing owed, the plan's pool for today is not used up,
/// and neither is the user's own share of it.
pub fn sponsor_of(state: &State, user: &Address, now_secs: u64) -> Option<Address> {
    let client = billing_client_of(state, user, now_secs)?;
    if client == *user || state.clients.usage_owed.contains_key(&client) {
        return None;
    }
    let plan = active_plan(state, &client, now_secs)?;
    let day = now_secs / 86_400;
    let pool_used = if plan.user_actions_day == day {
        plan.user_actions_used
    } else {
        0
    };
    let user_used = state
        .account_ref(user)
        .map(|m| m.budget.sponsored_on(day))
        .unwrap_or(0);
    (pool_used < plan.user_actions_per_day
        && user_used < state.clients.params.user_actions_per_user_per_day)
        .then_some(client)
}

/// Take one action for `user` from `client`'s plan (`sponsor_of` said it
/// may).
pub fn take_user_action(state: &mut State, client: Address, user: Address, now_secs: u64) {
    let day = now_secs / 86_400;
    if let Some(plan) = state.clients.plans.get_mut(&client) {
        if plan.user_actions_day != day {
            plan.user_actions_day = day;
            plan.user_actions_used = 0;
        }
        plan.user_actions_used = plan.user_actions_used.saturating_add(1);
    }
    state.account(user).budget.take_sponsored(day);
}

/// `SetClientFee`: an attester's retail schedule. Capped by governance
/// until the client keeps all its fees (`protocol_fee_waived`), after
/// which the schedule is the client's own business.
pub fn apply_set_fee(
    state: &mut State,
    _height: u64,
    signer: Address,
    fee: &ClientFee,
) -> Result<Vec<Event>, VmError> {
    if !state.attest.attesters.contains(&signer) {
        return Err(VmError::Unauthorized);
    }
    let p = &state.clients.params;
    let uncapped = p.protocol_fee_waived == 1;
    if !uncapped
        && (fee.p2p_bps > p.fee_cap_p2p_bps
            || fee.taker_bps > p.fee_cap_taker_bps
            || fee.withdraw_bps > p.fee_cap_withdraw_bps)
    {
        return Err(VmError::Invalid(format!(
            "retail fee above the caps ({} / {} / {} bps)",
            p.fee_cap_p2p_bps, p.fee_cap_taker_bps, p.fee_cap_withdraw_bps
        )));
    }
    state.clients.fees.insert(signer, fee.clone());
    Ok(vec![Event::ClientFeeSet {
        client: signer,
        p2p_bps: fee.p2p_bps,
        taker_bps: fee.taker_bps,
        withdraw_bps: fee.withdraw_bps,
    }])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    P2p,
    Taker,
    Withdraw,
}

impl Flow {
    fn name(self) -> &'static str {
        match self {
            Flow::P2p => "p2p",
            Flow::Taker => "taker",
            Flow::Withdraw => "withdraw",
        }
    }
}

/// The client that collects retail fees from `payer`, if any.
pub fn client_of(state: &State, payer: &Address) -> Option<Address> {
    state.clients.attested_by.get(payer).copied()
}

/// The client billed for `payer`'s flows: its attester, while the
/// attestation is unexpired. `client_of` never expires, which is right for
/// retail fees the client agreed with its user but not for billing.
pub fn billing_client_of(state: &State, payer: &Address, now_secs: u64) -> Option<Address> {
    let client = client_of(state, payer)?;
    let meta = state.account_ref(payer)?;
    (meta.tier_expires_at > now_secs).then_some(client)
}

/// `Some(client)` when the chain waives its protocol fee for `payer` and
/// bills that client instead: the pricing is switched on, the payer has a
/// live client that is not paying for itself, and that client has a
/// running plan and owes nothing.
pub fn waives_fee(state: &State, _height: u64, payer: &Address, now_secs: u64) -> Option<Address> {
    if state.clients.params.protocol_fee_waived != 1 {
        return None;
    }
    let client = billing_client_of(state, payer, now_secs)?;
    if client == *payer
        || active_plan(state, &client, now_secs).is_none()
        || state.clients.usage_owed.contains_key(&client)
    {
        return None;
    }
    Some(client)
}

/// Charge `payer`'s retail fee for `flow` on `base_amount` of `asset` and
/// credit it to its client, up to what the payer's deposit account holds.
/// One balanced posting under `group`; nothing happens when the payer has
/// no client, the client has no schedule, or the fee rounds to zero.
#[allow(clippy::too_many_arguments)]
pub fn retail(
    state: &mut State,
    external_id: &str,
    tx_type: TxType,
    group: Option<&str>,
    payer: Address,
    asset: &Asset,
    base_amount: Amount,
    flow: Flow,
) -> Result<Vec<Event>, VmError> {
    let Some(client) = client_of(state, &payer) else {
        return Ok(Vec::new());
    };
    if client == payer {
        return Ok(Vec::new());
    }
    let Some(schedule) = state.clients.fees.get(&client) else {
        return Ok(Vec::new());
    };
    let bps = match flow {
        Flow::P2p => schedule.p2p_bps,
        Flow::Taker => schedule.taker_bps,
        Flow::Withdraw => schedule.withdraw_bps,
    };
    let due = mul_div_floor(base_amount, bps as Amount, 10_000).unwrap_or(0);
    let available = state
        .ledger
        .balance(&tokens::deposit_key(payer, asset))
        .max(0) as Amount;
    let amount = due.min(available);
    if amount == 0 {
        return Ok(Vec::new());
    }
    state.ledger.post(
        external_id,
        tx_type,
        group,
        None,
        vec![
            Record::debit(tokens::deposit_key(payer, asset), amount),
            Record::credit(tokens::deposit_key(client, asset), amount),
        ],
    )?;
    let e = state
        .clients
        .earned
        .entry((client, asset.clone()))
        .or_insert(0);
    *e = e.saturating_add(amount);
    Ok(vec![Event::ClientFeePaid {
        client,
        payer,
        asset: asset.clone(),
        amount,
        flow: flow.name().to_string(),
    }])
}

/// The usage price a client pays when one of its accounts uses a custody
/// service (`kind`: address | outbound). Dollars, settled in the stable
/// asset through the fee split; an unfunded client fails the user's
/// action.
pub fn charge_usage(
    state: &mut State,
    user: Address,
    kind: &str,
    tx_id: &[u8; 32],
) -> Result<Vec<Event>, VmError> {
    let Some(client) = client_of(state, &user) else {
        return Ok(Vec::new());
    };
    charge_usage_to(state, client, kind, tx_id)
}

fn note_paid(state: &mut State, client: Address, asset: &Asset, amount: Amount) {
    let u = state
        .clients
        .usage_paid
        .entry((client, asset.clone()))
        .or_insert(0);
    *u = u.saturating_add(amount);
}

/// Charge the usage price of `kind` to `client` directly (custody
/// services are billed to the custodian whoever signed).
///
/// The price is in USD micro, which is the stable asset's smallest unit.
/// A running plan's allowance pays first; the rest comes out of the
/// client's stable balance.
///
/// `address`, `outbound`, `swap` and `rotation` are the rails: a client
/// that cannot pay the price and everything it owes fails the user's
/// action, and nothing is taken. `fill`, `release` and `ruling` are the
/// flows whose protocol fee the chain waived for the client's user: those
/// never block the user; what the client cannot pay is recorded as owed
/// and collected from its next stable balance.
pub fn charge_usage_to(
    state: &mut State,
    client: Address,
    kind: &str,
    tx_id: &[u8; 32],
) -> Result<Vec<Event>, VmError> {
    let p = &state.clients.params;
    let (price, soft) = match kind {
        "address" => (p.usage_address_usd_micro, false),
        // A float swap is rails usage like a payout, priced the same until
        // it gets its own knob.
        "outbound" | "swap" => (p.usage_outbound_usd_micro, false),
        "rotation" => (state.pools.params.usage_rotation_usd_micro, false),
        "fill" => (p.usage_fill_usd_micro, true),
        "release" => (p.usage_release_usd_micro, true),
        "ruling" => (p.usage_ruling_usd_micro, true),
        _ => (0, true),
    };
    let stable = state.tokens.stable.clone();
    let from = tokens::deposit_key(client, &stable);
    let now_secs = state.timestamp / 1000;
    let covered = active_plan(state, &client, now_secs)
        .map(|plan| plan.allowance_left_usd_micro.min(price))
        .unwrap_or(0);
    let due = price - covered;
    let owed = state.clients.usage_owed.get(&client).copied().unwrap_or(0);
    let have = state.ledger.balance(&from).max(0) as Amount;
    if !soft && have < owed.saturating_add(due) {
        // Nothing is posted or recorded: the action fails as a whole.
        return Err(VmError::Invalid(if owed > 0 {
            format!(
                "client {} owes usage; no {kind} until it is paid",
                client.to_hex()
            )
        } else {
            format!(
                "client {} cannot pay the {kind} usage price of {due} USD micro",
                client.to_hex()
            )
        }));
    }
    let mut events = Vec::new();
    // Settle what is owed first, as far as the balance goes.
    let pay_owed = owed.min(have);
    if pay_owed > 0 {
        fees::collect(
            state,
            &format!("usage:owed:{}", tokens::hex(tx_id)),
            TxType::BudgetPurchase,
            None,
            from.clone(),
            &stable,
            pay_owed,
        )?;
        if owed == pay_owed {
            state.clients.usage_owed.remove(&client);
        } else {
            state.clients.usage_owed.insert(client, owed - pay_owed);
        }
        note_paid(state, client, &stable, pay_owed);
        events.push(Event::UsageCharged {
            client,
            asset: stable.clone(),
            amount: pay_owed,
            kind: "owed".into(),
        });
    }
    if covered > 0 {
        if let Some(plan) = state.clients.plans.get_mut(&client) {
            plan.allowance_left_usd_micro -= covered;
        }
        events.push(Event::UsageCovered {
            client,
            amount: covered,
            kind: kind.to_string(),
        });
    }
    if due == 0 {
        return Ok(events);
    }
    let left = have - pay_owed;
    if left >= due {
        fees::collect(
            state,
            &format!("usage:{kind}:{}", tokens::hex(tx_id)),
            TxType::BudgetPurchase,
            None,
            from,
            &stable,
            due,
        )?;
        note_paid(state, client, &stable, due);
        events.push(Event::UsageCharged {
            client,
            asset: stable,
            amount: due,
            kind: kind.to_string(),
        });
    } else {
        // Only a soft charge gets here (a hard one was refused above).
        let o = state.clients.usage_owed.entry(client).or_insert(0);
        *o = o.saturating_add(due);
        events.push(Event::UsageOwed {
            client,
            amount: due,
            kind: kind.to_string(),
        });
    }
    Ok(events)
}

/// Every block: renew the plans whose period ended. At an epoch boundary,
/// before that: collect usage owed from clients that hold the stable asset
/// again.
pub fn end_block(state: &mut State, ctx: &crate::context::BlockContext) -> Vec<Event> {
    let len = state.params.epoch_length_blocks.max(1);
    let mut events = Vec::new();
    if ctx.height != 0 && ctx.height.is_multiple_of(len) {
        events.extend(collect_owed(state, ctx));
    }
    events.extend(renew_plans(state, ctx));
    events
}

fn collect_owed(state: &mut State, ctx: &crate::context::BlockContext) -> Vec<Event> {
    let mut events = Vec::new();
    let stable = state.tokens.stable.clone();
    let owing: Vec<(Address, Amount)> = state
        .clients
        .usage_owed
        .iter()
        .map(|(c, a)| (*c, *a))
        .collect();
    for (client, owed) in owing {
        let have = state
            .ledger
            .balance(&tokens::deposit_key(client, &stable))
            .max(0) as Amount;
        let pay = owed.min(have);
        if pay == 0 {
            continue;
        }
        if fees::collect(
            state,
            &format!("usage:owed:{}:{}", ctx.height, client),
            TxType::BudgetPurchase,
            None,
            tokens::deposit_key(client, &stable),
            &stable,
            pay,
        )
        .is_err()
        {
            continue;
        }
        let left = owed - pay;
        if left == 0 {
            state.clients.usage_owed.remove(&client);
        } else {
            state.clients.usage_owed.insert(client, left);
        }
        note_paid(state, client, &stable, pay);
        events.push(Event::UsageCharged {
            client,
            asset: stable.clone(),
            amount: pay,
            kind: "owed".into(),
        });
    }
    events
}

/// Start the next period of every plan that ended and is set to renew, at
/// its renewal tier or the highest tier below it the client's lock still
/// meets. The fee (and anything owed) is paid in full from the client's
/// stable balance; a client that cannot pay, or whose lock meets no tier,
/// has no plan and is tried again every block until it can.
fn renew_plans(state: &mut State, ctx: &crate::context::BlockContext) -> Vec<Event> {
    let now = ctx.seconds();
    let due: Vec<(Address, u8)> = state
        .clients
        .plans
        .iter()
        .filter(|(_, p)| p.renew_tier > 0 && now >= p.ends_secs)
        .map(|(c, p)| (*c, p.renew_tier))
        .collect();
    let mut events = Vec::new();
    for (client, renew_tier) in due {
        if !state.attest.attesters.contains(&client) {
            continue;
        }
        let tier =
            tier_for_lock(&state.clients.params, locked_keel(state, &client)).min(renew_tier);
        if tier == 0 {
            continue;
        }
        let fee = state.clients.params.tiers[tier as usize - 1].fee_usd_micro;
        if let Ok(mut started) = start_plan(
            state,
            client,
            tier,
            fee,
            now,
            &format!("plan:{}:{}", ctx.height, client),
        ) {
            // A renewal keeps the tier the client asked to renew at, even
            // when a smaller lock gave it a lower one this time.
            if let Some(plan) = state.clients.plans.get_mut(&client) {
                plan.renew_tier = renew_tier;
            }
            events.append(&mut started);
        }
    }
    events
}
