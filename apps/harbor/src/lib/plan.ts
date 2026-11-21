/**
 * The billing model as pure functions over the client record: where the
 * plan stands, what each tier would take to buy, the credit an upgrade
 * gets, and the one thing the client should do next. The chain is the
 * authority (`chain/crates/keel-vm/src/modules/clients.rs`); this mirrors
 * its rules so the page can say what a signature will do before it is
 * asked for.
 */
import type { ClientRecord, Plan, Tier } from '../api/types';
import { big, keel, money, shortfall } from './amounts';

export type PlanStatus = 'none' | 'active' | 'suspended' | 'expired';

/** What the client holds outside the lock, in smallest units. */
export interface Funds {
  usdt: bigint;
  keel: bigint;
}

const DAY = 86_400;
/** How long before the period ends the renewal becomes the next thing to look at. */
export const RENEWAL_NOTICE_SECS = 7 * DAY;

export function planStatus(c: ClientRecord): PlanStatus {
  const p = c.plan;
  if (!p) return 'none';
  if (p.active) return 'active';
  if (p.suspended) return 'suspended';
  return 'expired';
}

/** A period is running (active or suspended): only a higher tier can be bought. */
export function isRunning(c: ClientRecord): boolean {
  const s = planStatus(c);
  return s === 'active' || s === 'suspended';
}

/** Tiers governance has switched on (a tier with no lock is not for sale), lowest first. */
export function offeredTiers(c: Pick<ClientRecord, 'tiers'>): Tier[] {
  return c.tiers.filter((t) => big(t.lock_keel) > 0n).sort((a, b) => a.tier - b.tier);
}

export function tierOf(c: Pick<ClientRecord, 'tiers'>, n: number): Tier | undefined {
  return offeredTiers(c).find((t) => t.tier === n);
}

export function secondsLeft(plan: Plan, nowSecs: number): number {
  return Math.max(0, plan.ends_at - nowSecs);
}

/** Whole days left, rounded up: a period ending in 26 hours has 2 days left. */
export function daysLeft(plan: Plan, nowSecs: number): number {
  return Math.ceil(secondsLeft(plan, nowSecs) / DAY);
}

/**
 * What a running period is still worth when the client upgrades, in USD
 * micro: the fee times the smaller of the share of time left and the share
 * of allowance left, each floored as the chain floors them.
 */
export function upgradeCredit(plan: Plan, nowSecs: number): bigint {
  const length = BigInt(Math.max(0, plan.ends_at - plan.started_at));
  const left = BigInt(secondsLeft(plan, nowSecs));
  if (length === 0n) return 0n;
  const fee = big(plan.fee_usd_micro);
  const byTime = (fee * left) / length;
  const allowance = big(plan.allowance_usd_micro);
  const byUsage = allowance === 0n ? byTime : (fee * big(plan.allowance_left_usd_micro)) / allowance;
  return byTime < byUsage ? byTime : byUsage;
}

export interface TierOffer {
  tier: Tier;
  /**
   * `current`: the running plan. `lower`: below the running plan, so only
   * reachable as a renewal. `buy`: no period is running. `upgrade`: above
   * the running plan.
   */
  kind: 'current' | 'lower' | 'buy' | 'upgrade';
  /** Orcas still to lock before this tier can be bought. */
  needLock: bigint;
  /** Billing asset still to add before the charge can be paid. */
  needUsdt: bigint;
  /** Taken off the fee for what the running period had left (upgrades only). */
  credit: bigint;
  /** The fee after the credit. */
  fee: bigint;
  /** Usage owed, which the same action settles first. */
  owed: bigint;
  /** What signing `BuyPlan` takes from the balance now: fee plus owed. */
  charge: bigint;
  canBuy: boolean;
}

/** What buying `tier` would take right now. */
export function tierOffer(c: ClientRecord, funds: Funds, tier: Tier, nowSecs: number): TierOffer {
  const plan = c.plan;
  const running = plan !== null && isRunning(c);
  const owed = big(c.usage_owed);
  const full = big(tier.fee_usd_micro);
  const kind: TierOffer['kind'] = !running ? 'buy' : tier.tier === plan.tier ? 'current' : tier.tier < plan.tier ? 'lower' : 'upgrade';
  const credit = kind === 'upgrade' ? upgradeCredit(plan!, nowSecs) : 0n;
  const fee = shortfall(full, credit);
  const charge = fee + owed;
  const needLock = shortfall(big(tier.lock_keel), big(c.locked_keel));
  const needUsdt = shortfall(charge, funds.usdt);
  const buyable = kind === 'buy' || kind === 'upgrade';
  return { tier, kind, needLock, needUsdt, credit, fee, owed, charge, canBuy: c.attester && buyable && needLock === 0n && needUsdt === 0n };
}

export type NextStep =
  /** Lock more KEEL: to restore a suspended plan, to buy, or to renew. */
  | { kind: 'lock'; amount: bigint; tier: number; why: 'restore' | 'buy' | 'renew' }
  /** Add the billing asset: to settle what is owed, to buy, or to renew. */
  | { kind: 'fund'; amount: bigint; tier: number | null; why: 'owed' | 'buy' | 'renew' }
  | { kind: 'buy'; tier: number; charge: bigint }
  /** Lock and balance are in place, but the account is not registered as a client yet. */
  | { kind: 'register'; tier: number }
  /** The period ends soon and renewal is off. */
  | { kind: 'ends'; endsAt: number }
  /** Nothing to sign: the chain acts by itself in a coming block. */
  | { kind: 'wait'; why: 'renewing' | 'collecting' }
  | { kind: 'none' };

/**
 * The single next thing to do. Order: restore a suspended plan, settle
 * what is owed, get a plan, then keep the running plan renewable.
 */
export function nextStep(c: ClientRecord, funds: Funds, nowSecs: number): NextStep {
  const tiers = offeredTiers(c);
  const status = planStatus(c);
  const plan = c.plan;
  const owed = big(c.usage_owed);
  const locked = big(c.locked_keel);

  if (status === 'suspended' && plan) {
    return { kind: 'lock', amount: shortfall(big(plan.lock_keel), locked), tier: plan.tier, why: 'restore' };
  }
  if (owed > funds.usdt) {
    return { kind: 'fund', amount: owed - funds.usdt, tier: null, why: 'owed' };
  }
  if (status === 'expired' && plan && plan.renew_tier > 0) {
    // The chain renews at the renewal tier, or the highest tier below it
    // the lock still meets, in the first block the balance covers it.
    const lockTier = Math.min(c.tier, plan.renew_tier);
    const target = tierOf(c, lockTier > 0 ? lockTier : plan.renew_tier);
    if (target) {
      const needLock = shortfall(big(target.lock_keel), locked);
      if (needLock > 0n) return { kind: 'lock', amount: needLock, tier: target.tier, why: 'renew' };
      const needUsdt = shortfall(big(target.fee_usd_micro) + owed, funds.usdt);
      if (needUsdt > 0n) return { kind: 'fund', amount: needUsdt, tier: target.tier, why: 'renew' };
      return { kind: 'wait', why: 'renewing' };
    }
  }
  if (status === 'none' || status === 'expired') {
    const target = tierOf(c, c.tier) ?? tiers[0];
    if (!target) return { kind: 'none' };
    const offer = tierOffer(c, funds, target, nowSecs);
    if (offer.needLock > 0n) return { kind: 'lock', amount: offer.needLock, tier: target.tier, why: 'buy' };
    if (offer.needUsdt > 0n) return { kind: 'fund', amount: offer.needUsdt, tier: target.tier, why: 'buy' };
    if (!c.attester) return { kind: 'register', tier: target.tier };
    return { kind: 'buy', tier: target.tier, charge: offer.charge };
  }
  if (plan && secondsLeft(plan, nowSecs) <= RENEWAL_NOTICE_SECS) {
    if (plan.renew_tier === 0) return { kind: 'ends', endsAt: plan.ends_at };
    const target = tierOf(c, plan.renew_tier);
    if (target) {
      const needLock = shortfall(big(target.lock_keel), locked);
      if (needLock > 0n) return { kind: 'lock', amount: needLock, tier: target.tier, why: 'renew' };
      const needUsdt = shortfall(big(target.fee_usd_micro) + owed, funds.usdt);
      if (needUsdt > 0n) return { kind: 'fund', amount: needUsdt, tier: target.tier, why: 'renew' };
    }
  }
  if (owed > 0n) return { kind: 'wait', why: 'collecting' };
  return { kind: 'none' };
}

export interface StepText {
  title: string;
  detail: string;
  /** The Harbor page where it is done; null when there is nothing to do. */
  page: 'funds' | 'plans' | null;
}

function day(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

/** A step in words: the title is the button, the detail says why. */
export function stepText(step: NextStep, c: ClientRecord): StepText {
  const asset = c.billing_asset;
  switch (step.kind) {
    case 'lock':
      return {
        title: `Lock ${keel(step.amount)}`,
        detail:
          step.why === 'restore'
            ? `Your tier ${step.tier} plan is suspended because the lock fell below what it was bought with. Locking the difference restores it at once.`
            : step.why === 'renew'
              ? `Tier ${step.tier} needs this much more locked to renew.`
              : `Tier ${step.tier} needs this much more locked before you can buy it.`,
        page: 'funds',
      };
    case 'fund':
      return {
        title: `Add ${money(step.amount, asset)}`,
        detail:
          step.why === 'owed'
            ? 'Usage is owed. Until it is paid your users pay protocol fees again and no deposit addresses or payouts are issued to them.'
            : step.why === 'renew'
              ? `The balance does not cover the tier ${step.tier ?? ''} renewal yet.`
              : `The balance does not cover the tier ${step.tier ?? ''} fee yet.`,
        page: 'funds',
      };
    case 'buy':
      return { title: `Buy tier ${step.tier}`, detail: `The lock and the balance are in place. Buying takes ${money(step.charge, asset)} and starts the period in that block.`, page: 'plans' };
    case 'register':
      return {
        title: 'Waiting for registration',
        detail: `The lock and the balance for tier ${step.tier} are in place. The plan can be bought with this same account as soon as governance has registered it as a client.`,
        page: null,
      };
    case 'ends':
      return { title: 'Turn renewal back on', detail: `The plan ends on ${day(step.endsAt)} and is set not to renew. After that your users pay protocol fees.`, page: 'plans' };
    case 'wait':
      return step.why === 'renewing'
        ? { title: 'Renewal is under way', detail: 'The lock and the balance are in place; the chain starts the next period in a coming block.', page: null }
        : { title: 'Nothing to sign', detail: 'The balance covers what is owed; the chain collects it at the next epoch boundary or with the next charge.', page: null };
    case 'none':
      return { title: 'All set', detail: 'The plan is running and nothing is owed.', page: null };
  }
}

/** A period length in words: "30 days". */
export function periodLabel(secs: number): string {
  if (secs >= DAY && secs % DAY === 0) return secs === DAY ? '1 day' : `${secs / DAY} days`;
  if (secs >= 3600) return `${Math.round(secs / 3600)} hours`;
  return `${Math.round(secs / 60)} minutes`;
}
