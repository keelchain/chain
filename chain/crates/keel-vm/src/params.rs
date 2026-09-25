//! Governance parameters. Every knob the backoffice had is a numeric field
//! here, changeable by a `ParamChange` proposal keyed by field name.

use borsh::{BorshDeserialize, BorshSerialize};
use keel_actions::BudgetParams;
use serde::{Deserialize, Serialize};

macro_rules! params {
    ($($(#[$m:meta])* $name:ident : $ty:ty = $default:expr),* $(,)?) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
        #[serde(default)]
        pub struct Params {
            $($(#[$m])* pub $name: $ty,)*
            pub budget: BudgetParams,
        }

        impl Default for Params {
            fn default() -> Self {
                Self { $($name: $default,)* budget: BudgetParams::default() }
            }
        }

        impl Params {
            pub const KEYS: &'static [&'static str] = &[$(stringify!($name),)* "budget.base", "budget.per_usd_filled", "budget.cancel_bonus", "budget.max_per_block", "budget.price_per_action", "budget.per_locked_keel_per_day", "budget.unlock_delay_secs"];

            pub fn get(&self, key: &str) -> Option<u128> {
                match key {
                    $(stringify!($name) => Some(self.$name as u128),)*
                    "budget.base" => Some(self.budget.base as u128),
                    "budget.per_usd_filled" => Some(self.budget.per_usd_filled as u128),
                    "budget.cancel_bonus" => Some(self.budget.cancel_bonus as u128),
                    "budget.max_per_block" => Some(self.budget.max_per_block as u128),
                    "budget.price_per_action" => Some(self.budget.price_per_action),
                    "budget.per_locked_keel_per_day" => Some(self.budget.per_locked_keel_per_day as u128),
                    "budget.unlock_delay_secs" => Some(self.budget.unlock_delay_secs as u128),
                    _ => None,
                }
            }

            /// Set by name. Returns false for unknown keys or out-of-range values.
            pub fn set(&mut self, key: &str, value: u128) -> bool {
                match key {
                    $(stringify!($name) => { match <$ty>::try_from(value) { Ok(v) => { self.$name = v; true } Err(_) => false } })*
                    "budget.base" => { match u64::try_from(value) { Ok(v) => { self.budget.base = v; true } Err(_) => false } }
                    "budget.per_usd_filled" => { match u64::try_from(value) { Ok(v) => { self.budget.per_usd_filled = v; true } Err(_) => false } }
                    "budget.cancel_bonus" => { match u64::try_from(value) { Ok(v) => { self.budget.cancel_bonus = v; true } Err(_) => false } }
                    "budget.max_per_block" => { match u32::try_from(value) { Ok(v) => { self.budget.max_per_block = v; true } Err(_) => false } }
                    "budget.price_per_action" => { self.budget.price_per_action = value; true }
                    "budget.per_locked_keel_per_day" => { match u64::try_from(value) { Ok(v) => { self.budget.per_locked_keel_per_day = v; true } Err(_) => false } }
                    "budget.unlock_delay_secs" => { match u64::try_from(value) { Ok(v) if v <= 365 * 86_400 => { self.budget.unlock_delay_secs = v; true } _ => false } }
                    _ => false,
                }
            }
        }
    };
}

params! {
    // ---- block production (2026-09-08: block time set from the
    // backoffice, starting slow and tightening later; both take effect
    // for proposals immediately and for consensus timeouts at the next epoch)
    /// Minimum milliseconds between blocks while transactions are pending.
    /// The floor is the network's two quorum round trips: below it the
    /// leader timeout fires and blocks get slower, not faster.
    min_block_interval_ms: u32 = 500,
    /// Milliseconds between blocks while the mempool is empty. Idle blocks
    /// carry only certificates; a long idle interval keeps disk growth
    /// proportional to activity.
    idle_block_interval_ms: u32 = 5_000,
    // ---- fees (basis points unless stated)
    /// Order-book taker fee (launch proposal 10 bps; today's off-chain 20).
    taker_fee_bps: u32 = 10,
    maker_fee_bps: u32 = 0,
    /// P2P trade fee charged to the seller on release.
    p2p_seller_fee_bps: u32 = 100,
    /// Extra fee on trades below `p2p_small_trade_usd`.
    p2p_small_trade_surcharge_bps: u32 = 100,
    /// USD threshold (micro-units) for the small-trade surcharge.
    p2p_small_trade_usd_micro: u64 = 50_000_000,
    /// Share of the seller fee paid to a referrer, when the offer names one.
    referral_share_bps: u32 = 2_000,
    /// Flat withdrawal fee in USD micro-units, on top of network cost.
    withdraw_flat_fee_usd_micro: u64 = 1_000_000,
    /// Dispute fee charged to the losing side.
    dispute_fee_bps: u32 = 100,
    /// Fee split: must sum to 10_000.
    fee_split_treasury_bps: u32 = 5_000,
    fee_split_validators_bps: u32 = 4_000,
    fee_split_burn_bps: u32 = 1_000,
    // ---- P2P
    /// Refundable KEEL deposit (smallest units) per live offer.
    offer_deposit: u128 = 10_000_000,
    /// Seconds a buyer has to mark paid before the seller may cancel.
    default_payment_window_secs: u32 = 1_800,
    /// Seconds after "paid" before the buyer may open a dispute.
    release_grace_secs: u32 = 3_600,
    /// Seconds an arbitrator has to rule before another may.
    ruling_window_secs: u32 = 86_400,
    /// Free offers per address, then +`offer_allowance_step` per N trades.
    offer_allowance_base: u32 = 2,
    offer_allowance_trades_per_step: u32 = 10,
    offer_allowance_step: u32 = 2,
    // ---- staking
    min_validator_bond: u128 = 100_000_000_000,
    min_observer_bond: u128 = 500_000_000_000,
    min_arbitrator_bond: u128 = 50_000_000_000,
    /// Blocks between unbond and withdrawal.
    unbonding_blocks: u64 = 100_000,
    max_validators: u32 = 100,
    /// Validator-set / vault epoch length in blocks.
    epoch_length_blocks: u64 = 10_000,
    /// Share of validator rewards paid to observers before stake weighting.
    observer_reward_bps: u32 = 2_500,
    slash_double_sign_bps: u32 = 500,
    slash_false_observation_bps: u32 = 10_000,
    // ---- governance
    proposal_deposit: u128 = 1_000_000_000_000,
    voting_period_blocks: u64 = 20_000,
    timelock_blocks: u64 = 5_000,
    quorum_bps: u32 = 3_340,
    threshold_bps: u32 = 5_000,
    veto_bps: u32 = 3_340,
    // ---- vaults
    /// Observer attestations needed, in bps of observer bond.
    observer_quorum_bps: u32 = 6_667,
    confirmations_btc: u32 = 2,
    confirmations_eth: u32 = 12,
    confirmations_tron: u32 = 19,
    /// Per-chain daily credit cap in USD micro-units (0 = unlimited).
    deposit_daily_cap_usd_micro: u64 = 0,
    /// Deposits above this USD value become spendable only after
    /// `large_deposit_delay_blocks`.
    large_deposit_usd_micro: u64 = 50_000_000_000,
    large_deposit_delay_blocks: u64 = 600,
    /// Blocks between outbound batches per chain.
    outbound_batch_interval_blocks: u64 = 20,
    // ---- lightning (2026-09-10; every knob editable from the backoffice)
    /// 1 = Lightning deposits and withdrawals accepted.
    lightning_enabled: u32 = 1,
    /// Most BTC (sats) one observer may hold in its Lightning pool.
    lightning_pool_cap_sats: u64 = 50_000_000,
    /// Largest single Lightning deposit / withdrawal (sats).
    lightning_max_deposit_sats: u64 = 10_000_000,
    lightning_max_withdraw_sats: u64 = 10_000_000,
    /// Routing fee allowance charged to the withdrawer: max(bps of amount, floor).
    lightning_max_fee_bps: u32 = 50,
    lightning_min_fee_sats: u64 = 10,
    /// Blocks the assigned observer has to report a payout before it is refunded.
    lightning_payout_timeout_blocks: u64 = 600,
    /// Daily Lightning deposit cap per observer (sats, 0 = unlimited).
    lightning_daily_cap_sats: u64 = 0,
    // ---- markets
    /// Blocks a house quote stays valid at most.
    house_quote_max_ttl_blocks: u64 = 60,
    max_open_orders_per_pair: u32 = 500,
}

impl Params {
    /// Busy interval within [0, 60 s], idle interval within [busy, 60 s].
    pub fn block_timing_ok(&self) -> bool {
        self.min_block_interval_ms <= 60_000
            && self.idle_block_interval_ms <= 60_000
            && self.idle_block_interval_ms >= self.min_block_interval_ms
    }

    pub fn fee_split_ok(&self) -> bool {
        self.fee_split_treasury_bps + self.fee_split_validators_bps + self.fee_split_burn_bps
            == 10_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_set_by_name() {
        let mut p = Params::default();
        assert!(p.fee_split_ok());
        assert_eq!(p.get("taker_fee_bps"), Some(10));
        assert!(p.set("taker_fee_bps", 25));
        assert_eq!(p.taker_fee_bps, 25);
        assert!(!p.set("taker_fee_bps", u128::MAX));
        assert!(!p.set("nope", 1));
        assert!(p.set("budget.base", 5));
        assert_eq!(p.budget.base, 5);
        assert!(Params::KEYS.contains(&"budget.price_per_action"));
    }
}
