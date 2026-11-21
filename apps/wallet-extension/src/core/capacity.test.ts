import { describe, expect, it } from 'vitest';
import { encodeAction, type Action as SdkAction } from '@keelchain/sdk';
import { actionErrorText, capacityChangeProblem, lockBudgetAction, needsFirstLock, NO_CAPACITY, NO_CAPACITY_MESSAGE, parseCapacity, unlockArrival, unlockBudgetAction, type Capacity } from './capacity';
import { decodeAction } from './decode';

const KEEL = 1_000_000n;

describe('parseCapacity', () => {
  it('reads the budget object of the account payload', () => {
    const c = parseCapacity({
      nonce: 4,
      budget: { used: 0, limit: 0, remaining: 60, earned: 0, locked: '2000000', lock_cap_per_day: 200, lock_pool: 75, unlocking: [{ ready_at: 2_000, amount: '500000' }, { ready_at: 1_000, amount: '250000' }] },
    });
    expect(c).toEqual({ available: 75, locked: 2n * KEEL, perDay: 200, unlocking: [{ readyAt: 1_000, amount: 250_000n }, { readyAt: 2_000, amount: 500_000n }] });
  });

  it('adds the free budget left to the refilled pool', () => {
    expect(parseCapacity({ budget: { used: 3, limit: 10, remaining: 7, lock_pool: 40 } })?.available).toBe(47);
    expect(parseCapacity({ budget: { used: 12, limit: 10, remaining: 0, lock_pool: 5 } })?.available).toBe(5);
  });

  it('treats every field as optional', () => {
    expect(parseCapacity({ budget: {} })).toEqual(NO_CAPACITY);
    expect(parseCapacity({ budget: { remaining: 9 } })).toEqual({ available: 9, locked: 0n, perDay: 0, unlocking: [] });
    expect(parseCapacity({ budget: { locked: 'x', lock_cap_per_day: -1, remaining: null, unlocking: 'no' } })).toEqual(NO_CAPACITY);
  });

  it('finds the unlocks at the top level too, and skips rows it cannot read', () => {
    const c = parseCapacity({ budget: { locked: '1000000' }, unlocking: [{ ready_at: 5, amount: 7 }, { ready_at: 'soon', amount: '1' }, { ready_at: 6, amount: '0' }, null] });
    expect(c?.unlocking).toEqual([{ readyAt: 5, amount: 7n }]);
  });

  it('is null without a budget object', () => {
    expect(parseCapacity({ nonce: 1, balances: [] })).toBeNull();
    expect(parseCapacity({ budget: [] })).toBeNull();
    expect(parseCapacity(null)).toBeNull();
  });
});

describe('needsFirstLock', () => {
  it('is true only with no capacity and nothing locked', () => {
    expect(needsFirstLock(NO_CAPACITY)).toBe(true);
    expect(needsFirstLock({ ...NO_CAPACITY, locked: KEEL })).toBe(false);
    expect(needsFirstLock({ ...NO_CAPACITY, available: 1 })).toBe(false);
  });
});

describe('lock and unlock actions', () => {
  it('build the two variants in orcas, as the SDK encodes them', () => {
    expect(lockBudgetAction(KEEL)).toEqual({ LockBudget: { amount: 1_000_000n } });
    expect(unlockBudgetAction(250_000n)).toEqual({ UnlockBudget: { amount: 250_000n } });
    const lock = encodeAction(lockBudgetAction(KEEL) as unknown as SdkAction).bytes();
    const unlock = encodeAction(unlockBudgetAction(KEEL) as unknown as SdkAction).bytes();
    // A one-byte variant tag, then the amount as a little-endian u128.
    expect(lock.length).toBe(17);
    expect(Array.from(lock.slice(1, 5))).toEqual([0x40, 0x42, 0x0f, 0x00]);
    expect(Array.from(unlock.slice(1))).toEqual(Array.from(lock.slice(1)));
    expect(unlock[0]).not.toBe(lock[0]);
  });

  it('decode to the summaries the wallet shows', () => {
    expect(decodeAction(lockBudgetAction(KEEL)).summary).toBe('Lock 1 KEEL as action budget');
    expect(decodeAction(unlockBudgetAction(2n * KEEL)).summary).toBe('Unlock 2 KEEL of action budget');
  });
});

describe('capacityChangeProblem', () => {
  const some: Capacity = { available: 10, locked: 2n * KEEL, perDay: 200, unlocking: [] };

  it('accepts a first lock of 1 KEEL the account can pay', () => {
    expect(capacityChangeProblem('lock', KEEL, NO_CAPACITY, KEEL)).toBeNull();
  });

  it('refuses a lock below 1 KEEL from an account with no capacity', () => {
    expect(capacityChangeProblem('lock', KEEL / 2n, NO_CAPACITY, 5n * KEEL)).toMatch(/at least 1 KEEL/);
    expect(capacityChangeProblem('lock', KEEL / 2n, some, 5n * KEEL)).toBeNull();
  });

  it('refuses a lock above the available balance', () => {
    expect(capacityChangeProblem('lock', 3n * KEEL, some, 2n * KEEL)).toBe('More than the 2 KEEL available.');
  });

  it('refuses an unlock above the lock, with nothing locked, or with no capacity', () => {
    expect(capacityChangeProblem('unlock', 3n * KEEL, some, 0n)).toBe('More than the 2 KEEL locked.');
    expect(capacityChangeProblem('unlock', KEEL, NO_CAPACITY, 0n)).toBe('Nothing is locked.');
    expect(capacityChangeProblem('unlock', KEEL, { ...some, available: 0 }, 0n)).toMatch(/none are left/);
    expect(capacityChangeProblem('unlock', 2n * KEEL, some, 0n)).toBeNull();
  });

  it('refuses a missing or zero amount', () => {
    expect(capacityChangeProblem('lock', null, some, KEEL)).toMatch(/Enter an amount/);
    expect(capacityChangeProblem('unlock', 0n, some, KEEL)).toMatch(/Enter an amount/);
  });
});

describe('unlockArrival', () => {
  it('says how long is left', () => {
    const now = 1_800_000_000_000;
    expect(unlockArrival(now + 3 * 86_400_000, now)).toBe('in 3 d');
    expect(unlockArrival(now + 2 * 3_600_000, now)).toBe('in 2 h');
    expect(unlockArrival(now + 30_000, now)).toBe('in 30s');
    expect(unlockArrival(now, now)).toBe('arriving now');
    expect(unlockArrival(now - 1, now)).toBe('arriving now');
  });
});

describe('actionErrorText', () => {
  it('points BUDGET_EXHAUSTED to the capacity card', () => {
    expect(actionErrorText('BUDGET_EXHAUSTED', 'action budget exhausted')).toBe(NO_CAPACITY_MESSAGE);
    expect(NO_CAPACITY_MESSAGE).toBe('This account has no action capacity. Lock 1 KEEL on Home to get 100 actions a day.');
  });

  it('keeps the node message for other codes', () => {
    expect(actionErrorText('BAD_NONCE', 'bad nonce: expected 2, got 1')).toBe('bad nonce: expected 2, got 1');
    expect(actionErrorText(undefined, undefined)).toBe('rejected');
  });
});
