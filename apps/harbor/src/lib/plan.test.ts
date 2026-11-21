import { describe, expect, it } from 'vitest';
import { MOCK_NOW_SECS as NOW, MOCK_TIERS, mockClient, mockPlan } from '../api/mock';
import { daysLeft, nextStep, offeredTiers, periodLabel, planStatus, stepText, tierOffer, upgradeCredit, type Funds } from './plan';

const KEEL = 1_000_000n;
const USD = 1_000_000n;
const DAY = 86_400;
const funds = (usdt: bigint, keel = 0n): Funds => ({ usdt: usdt * USD, keel: keel * KEEL });
const locked = (n: bigint) => String(n * KEEL);

describe('planStatus', () => {
  it('reads none, active, suspended and expired off the record', () => {
    expect(planStatus(mockClient())).toBe('none');
    expect(planStatus(mockClient({ plan: mockPlan(1, 3) }))).toBe('active');
    expect(planStatus(mockClient({ plan: mockPlan(1, 3, { active: false, suspended: true }) }))).toBe('suspended');
    expect(planStatus(mockClient({ plan: mockPlan(1, 31, { active: false }) }))).toBe('expired');
  });
  it('rounds the days left up and never goes below zero', () => {
    expect(daysLeft(mockPlan(1, 12), NOW)).toBe(18);
    expect(daysLeft(mockPlan(1, 0, { ends_at: NOW + DAY + 3600 }), NOW)).toBe(2);
    expect(daysLeft(mockPlan(1, 31), NOW)).toBe(0);
  });
});

describe('offeredTiers', () => {
  it('leaves out a tier governance switched off (no lock) and names a period', () => {
    const tiers = [MOCK_TIERS[1]!, { ...MOCK_TIERS[2]!, lock_keel: '0' }, MOCK_TIERS[0]!];
    expect(offeredTiers({ tiers }).map((t) => t.tier)).toEqual([1, 2]);
    expect(periodLabel(30 * DAY)).toBe('30 days');
    expect(periodLabel(6 * 3600)).toBe('6 hours');
  });
});

describe('upgradeCredit', () => {
  it('is the fee times the share of time left when usage is lighter than time', () => {
    // 30-day tier 1 at $300, 10 days in, allowance untouched: 20/30 of the fee.
    expect(upgradeCredit(mockPlan(1, 10), NOW)).toBe(200n * USD);
  });
  it('is the share of allowance left when that is smaller', () => {
    // 10 days in but only a quarter of the allowance is left.
    expect(upgradeCredit(mockPlan(1, 10, { allowance_left_usd_micro: String(75n * USD) }), NOW)).toBe(75n * USD);
  });
  it('floors like the chain and is zero for an ended or zero-length period', () => {
    const plan = mockPlan(1, 0, { started_at: NOW - 1, ends_at: NOW + 2, fee_usd_micro: '100' });
    expect(upgradeCredit(plan, NOW)).toBe(66n);
    expect(upgradeCredit(mockPlan(1, 31), NOW)).toBe(0n);
    expect(upgradeCredit(mockPlan(1, 0, { ends_at: NOW }), NOW)).toBe(0n);
  });
  it('uses time alone when the plan includes no usage', () => {
    expect(upgradeCredit(mockPlan(1, 15, { allowance_usd_micro: '0', allowance_left_usd_micro: '0' }), NOW)).toBe(150n * USD);
  });
});

describe('tierOffer', () => {
  it('says what a first purchase still needs', () => {
    const c = mockClient({ locked_keel: locked(60_000n) });
    const o = tierOffer(c, funds(120n), MOCK_TIERS[0]!, NOW);
    expect(o.kind).toBe('buy');
    expect(o.needLock).toBe(40_000n * KEEL);
    expect(o.needUsdt).toBe(180n * USD);
    expect(o.canBuy).toBe(false);
  });
  it('can buy once the lock and the balance are there, and settles what is owed in the same charge', () => {
    const c = mockClient({ locked_keel: locked(100_000n), tier: 1, usage_owed: String(20n * USD) });
    const o = tierOffer(c, funds(320n), MOCK_TIERS[0]!, NOW);
    expect(o.charge).toBe(320n * USD);
    expect(o.canBuy).toBe(true);
    expect(tierOffer(c, funds(319n), MOCK_TIERS[0]!, NOW).needUsdt).toBe(1n * USD);
  });
  it('marks the running tier current, lower tiers renewal-only, higher tiers upgrades with credit', () => {
    const c = mockClient({ locked_keel: locked(2_000_000n), tier: 3, plan: mockPlan(2, 15) });
    const [t1, t2, t3] = MOCK_TIERS.map((t) => tierOffer(c, funds(5_000n), t, NOW));
    expect([t1!.kind, t2!.kind, t3!.kind]).toEqual(['lower', 'current', 'upgrade']);
    expect(t1!.canBuy || t2!.canBuy).toBe(false);
    // Half of tier 2's $1,000 comes off tier 3's $3,000.
    expect(t3!.credit).toBe(500n * USD);
    expect(t3!.charge).toBe(2_500n * USD);
    expect(t3!.canBuy).toBe(true);
  });
  it('never charges below zero when the credit exceeds the new fee', () => {
    const plan = mockPlan(1, 0, { fee_usd_micro: String(9_000n * USD) });
    const c = mockClient({ locked_keel: locked(500_000n), tier: 2, plan });
    expect(tierOffer(c, funds(0n), MOCK_TIERS[1]!, NOW).charge).toBe(0n);
  });
  it('treats an expired plan as no plan', () => {
    const c = mockClient({ locked_keel: locked(100_000n), tier: 1, plan: mockPlan(1, 31, { active: false }) });
    expect(tierOffer(c, funds(300n), MOCK_TIERS[0]!, NOW).kind).toBe('buy');
  });
});

describe('nextStep', () => {
  it('walks a new client through lock, fund, buy', () => {
    const fresh = mockClient();
    expect(nextStep(fresh, funds(0n), NOW)).toEqual({ kind: 'lock', amount: 100_000n * KEEL, tier: 1, why: 'buy' });
    const lockedIn = mockClient({ locked_keel: locked(100_000n), tier: 1 });
    expect(nextStep(lockedIn, funds(0n), NOW)).toEqual({ kind: 'fund', amount: 300n * USD, tier: 1, why: 'buy' });
    expect(nextStep(lockedIn, funds(300n), NOW)).toEqual({ kind: 'buy', tier: 1, charge: 300n * USD });
    expect(stepText(nextStep(fresh, funds(0n), NOW), fresh).title).toBe('Lock 100,000 KEEL');
    expect(stepText(nextStep(lockedIn, funds(0n), NOW), lockedIn).title).toBe('Add 300 USDT');
    expect(stepText(nextStep(lockedIn, funds(300n), NOW), lockedIn).title).toBe('Buy tier 1');
  });
  it('aims at the highest tier the lock already meets', () => {
    const c = mockClient({ locked_keel: locked(600_000n), tier: 2 });
    expect(nextStep(c, funds(1_000n), NOW)).toEqual({ kind: 'buy', tier: 2, charge: 1_000n * USD });
  });
  it('restores a suspended plan before anything else', () => {
    const c = mockClient({ locked_keel: locked(60_000n), usage_owed: String(5n * USD), plan: mockPlan(1, 9, { active: false, suspended: true }) });
    expect(nextStep(c, funds(0n), NOW)).toEqual({ kind: 'lock', amount: 40_000n * KEEL, tier: 1, why: 'restore' });
  });
  it('asks for what is owed when the balance does not cover it, and waits when it does', () => {
    const c = mockClient({ locked_keel: locked(100_000n), tier: 1, usage_owed: String(37n * USD), plan: mockPlan(1, 5) });
    expect(nextStep(c, funds(10n), NOW)).toEqual({ kind: 'fund', amount: 27n * USD, tier: null, why: 'owed' });
    expect(nextStep(c, funds(37n), NOW)).toEqual({ kind: 'wait', why: 'collecting' });
  });
  it('is quiet while a funded plan is far from renewal', () => {
    const c = mockClient({ locked_keel: locked(100_000n), tier: 1, plan: mockPlan(1, 5) });
    expect(nextStep(c, funds(0n), NOW)).toEqual({ kind: 'none' });
  });
  it('looks at the renewal in the last week of the period', () => {
    const base = { locked_keel: locked(100_000n), tier: 1 };
    const soon = mockClient({ ...base, plan: mockPlan(1, 25) });
    expect(nextStep(soon, funds(120n), NOW)).toEqual({ kind: 'fund', amount: 180n * USD, tier: 1, why: 'renew' });
    expect(nextStep(soon, funds(300n), NOW)).toEqual({ kind: 'none' });
    const up = mockClient({ ...base, plan: mockPlan(1, 25, { renew_tier: 2 }) });
    expect(nextStep(up, funds(5_000n), NOW)).toEqual({ kind: 'lock', amount: 400_000n * KEEL, tier: 2, why: 'renew' });
    const off = mockClient({ ...base, plan: mockPlan(1, 25, { renew_tier: 0 }) });
    expect(nextStep(off, funds(0n), NOW)).toEqual({ kind: 'ends', endsAt: off.plan!.ends_at });
  });
  it('tells an expired plan set to renew what the renewal is waiting for', () => {
    const plan = mockPlan(2, 31, { active: false });
    // The lock still meets tier 1, so the chain would renew there: tier 1's fee is what is missing.
    const c = mockClient({ locked_keel: locked(100_000n), tier: 1, plan });
    expect(nextStep(c, funds(80n), NOW)).toEqual({ kind: 'fund', amount: 220n * USD, tier: 1, why: 'renew' });
    expect(nextStep(c, funds(300n), NOW)).toEqual({ kind: 'wait', why: 'renewing' });
    const unlocked = mockClient({ locked_keel: '0', tier: 0, plan });
    expect(nextStep(unlocked, funds(0n), NOW)).toEqual({ kind: 'lock', amount: 500_000n * KEEL, tier: 2, why: 'renew' });
  });
  it('offers a fresh purchase to an expired plan that does not renew', () => {
    const c = mockClient({ locked_keel: locked(100_000n), tier: 1, plan: mockPlan(1, 31, { active: false, renew_tier: 0 }) });
    expect(nextStep(c, funds(300n), NOW)).toEqual({ kind: 'buy', tier: 1, charge: 300n * USD });
  });
});
