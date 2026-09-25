//! Per-address action budgets (Hyperliquid's address limits + dYdX's
//! per-block caps). Budgets are on-chain state, so every validator applies
//! the same admission rule and the mempool can pre-check it.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct BudgetParams {
    /// Free actions every address starts with.
    pub base: u64,
    /// Extra actions earned per whole USD of filled volume.
    pub per_usd_filled: u64,
    /// Cancels are allowed up to `min(limit + cancel_bonus, 2 * limit)`.
    pub cancel_bonus: u64,
    /// Actions one address may include in one block.
    pub max_per_block: u32,
    /// KEEL (smallest units) per purchased action.
    pub price_per_action: u128,
    /// Actions per day granted per whole KEEL locked (2026-09-08, the
    /// Tron energy model: lock, do not pay; the lock comes back in full).
    pub per_locked_keel_per_day: u64,
    /// Seconds between an unlock request and the KEEL returning to the
    /// deposit account.
    pub unlock_delay_secs: u64,
}

impl Default for BudgetParams {
    fn default() -> Self {
        Self {
            base: 10_000,
            per_usd_filled: 1,
            cancel_bonus: 100_000,
            max_per_block: 200,
            price_per_action: 1_000, // 0.001 KEEL
            per_locked_keel_per_day: 100,
            unlock_delay_secs: 3 * 86_400,
        }
    }
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize,
)]
pub struct Budget {
    pub used: u64,
    /// Earned from fills (whole USD × per_usd_filled) and purchases.
    pub earned: u64,
    pub last_block: u64,
    pub used_in_block: u32,
    /// KEEL (smallest units) locked for capacity.
    pub locked: u128,
    /// Regenerating pool of actions granted by the lock: refills linearly
    /// to `lock_cap` over 24 h, spent after the free budget is exhausted.
    pub lock_pool: u64,
    /// Block time (ms) of the last pool update.
    pub lock_updated_ms: u64,
    /// Accrual below one action carried to the next update (cap × ms units,
    /// < one day), so frequent updates do not truncate away capacity.
    pub lock_carry: u64,
}

const DAY_MS: u128 = 86_400_000;
const KEEL_UNIT: u128 = 1_000_000;

impl Budget {
    pub fn limit(&self, p: &BudgetParams) -> u64 {
        p.base.saturating_add(self.earned)
    }

    /// Daily action capacity the lock grants (also the pool's ceiling).
    pub fn lock_cap(&self, p: &BudgetParams) -> u64 {
        let cap = self
            .locked
            .saturating_mul(u128::from(p.per_locked_keel_per_day))
            / KEEL_UNIT;
        cap.min(u128::from(u64::MAX)) as u64
    }

    /// Refill the pool for the time elapsed since the last update.
    pub fn regen(&mut self, p: &BudgetParams, now_ms: u64) {
        let cap = self.lock_cap(p);
        if now_ms > self.lock_updated_ms {
            let elapsed = u128::from(now_ms - self.lock_updated_ms);
            let units = u128::from(cap)
                .saturating_mul(elapsed)
                .saturating_add(u128::from(self.lock_carry));
            let add = units / DAY_MS;
            self.lock_carry = (units % DAY_MS) as u64;
            self.lock_pool = self
                .lock_pool
                .saturating_add(add.min(u128::from(u64::MAX)) as u64)
                .min(cap);
        }
        if cap == 0 {
            self.lock_carry = 0;
        }
        self.lock_pool = self.lock_pool.min(cap);
        self.lock_updated_ms = now_ms;
    }

    pub fn lock(&mut self, p: &BudgetParams, now_ms: u64, amount: u128) {
        self.regen(p, now_ms);
        self.locked = self.locked.saturating_add(amount);
    }

    /// Reduce the lock; the pool is clipped to the new ceiling.
    pub fn unlock(&mut self, p: &BudgetParams, now_ms: u64, amount: u128) {
        self.regen(p, now_ms);
        self.locked = self.locked.saturating_sub(amount);
        self.lock_pool = self.lock_pool.min(self.lock_cap(p));
    }

    pub fn cancel_limit(&self, p: &BudgetParams) -> u64 {
        let limit = self.limit(p);
        limit
            .saturating_add(p.cancel_bonus)
            .min(limit.saturating_mul(2))
    }

    /// Free budget left plus the lock pool (as of the last regen).
    pub fn remaining(&self, p: &BudgetParams) -> u64 {
        self.limit(p)
            .saturating_sub(self.used)
            .saturating_add(self.lock_pool)
    }

    /// Try to spend one action at `block` / `now_ms`. `is_cancel` uses the
    /// wider limit. The free budget goes first, then the lock pool.
    pub fn spend(
        &mut self,
        p: &BudgetParams,
        block: u64,
        now_ms: u64,
        is_cancel: bool,
    ) -> Result<(), BudgetError> {
        if self.last_block != block {
            self.last_block = block;
            self.used_in_block = 0;
        }
        if self.used_in_block >= p.max_per_block {
            return Err(BudgetError::BlockCap);
        }
        let limit = if is_cancel {
            self.cancel_limit(p)
        } else {
            self.limit(p)
        };
        if self.used < limit {
            self.used += 1;
        } else {
            if self.locked > 0 {
                self.regen(p, now_ms);
            }
            if self.lock_pool == 0 {
                return Err(BudgetError::Exhausted);
            }
            self.lock_pool -= 1;
        }
        self.used_in_block += 1;
        Ok(())
    }

    pub fn earn_from_fill(&mut self, p: &BudgetParams, usd_whole: u128) {
        let add = (usd_whole.min(u64::MAX as u128) as u64).saturating_mul(p.per_usd_filled);
        self.earned = self.earned.saturating_add(add);
    }

    pub fn purchase(&mut self, actions: u64) {
        self.earned = self.earned.saturating_add(actions);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BudgetError {
    #[error("action budget exhausted")]
    Exhausted,
    #[error("per-block action cap reached")]
    BlockCap,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_budget_then_cancels_still_allowed() {
        let p = BudgetParams {
            base: 3,
            cancel_bonus: 2,
            max_per_block: 10,
            ..Default::default()
        };
        let mut b = Budget::default();
        for i in 0..3 {
            b.spend(&p, 1, 0, false)
                .unwrap_or_else(|_| panic!("action {i}"));
        }
        assert_eq!(b.spend(&p, 1, 0, false), Err(BudgetError::Exhausted));
        // cancel limit = min(3+2, 6) = 5
        b.spend(&p, 1, 0, true).unwrap();
        b.spend(&p, 1, 0, true).unwrap();
        assert_eq!(b.spend(&p, 1, 0, true), Err(BudgetError::Exhausted));
        b.earn_from_fill(&p, 10);
        assert_eq!(b.remaining(&p), 8);
        b.spend(&p, 2, 0, false).unwrap();
    }

    #[test]
    fn locked_keel_grants_a_pool_that_regenerates_over_a_day() {
        let p = BudgetParams {
            base: 1,
            cancel_bonus: 0,
            max_per_block: 1_000,
            per_locked_keel_per_day: 100,
            ..Default::default()
        };
        let mut b = Budget::default();
        // 2 KEEL locked at t=0 → cap 200 actions/day; the pool starts empty
        // and fills at 200 per 24 h.
        b.lock(&p, 0, 2 * KEEL_UNIT);
        assert_eq!(b.lock_cap(&p), 200);
        b.spend(&p, 1, 0, false).unwrap(); // the free one
        assert_eq!(b.spend(&p, 1, 0, false), Err(BudgetError::Exhausted));
        // Six hours later: 50 actions available.
        let six_h = 6 * 3_600_000;
        for _ in 0..50 {
            b.spend(&p, 2, six_h, false).unwrap();
        }
        assert_eq!(b.spend(&p, 2, six_h, false), Err(BudgetError::Exhausted));
        // Two days later the pool is full at the cap, never above it.
        b.regen(&p, 3 * 86_400_000);
        assert_eq!(b.lock_pool, 200);
        assert_eq!(b.remaining(&p), 200);
        // Unlocking half clips the pool to the new ceiling.
        b.unlock(&p, 3 * 86_400_000, KEEL_UNIT);
        assert_eq!(
            (b.locked, b.lock_cap(&p), b.lock_pool),
            (KEEL_UNIT, 100, 100)
        );
        b.unlock(&p, 3 * 86_400_000, KEEL_UNIT);
        assert_eq!((b.locked, b.lock_pool), (0, 0));
    }

    #[test]
    fn per_block_cap_resets_each_block() {
        let p = BudgetParams {
            max_per_block: 2,
            ..Default::default()
        };
        let mut b = Budget::default();
        b.spend(&p, 5, 0, false).unwrap();
        b.spend(&p, 5, 0, false).unwrap();
        assert_eq!(b.spend(&p, 5, 0, false), Err(BudgetError::BlockCap));
        b.spend(&p, 6, 0, false).unwrap();
    }
}
