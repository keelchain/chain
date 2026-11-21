/**
 * Action capacity: on a network an address sends actions on capacity granted
 * by KEEL it has locked. Each whole KEEL locked grants a number of actions
 * per day; the first day's worth is there at once and the pool then refills
 * over 24 hours. Unlocking returns the KEEL in full after a delay.
 *
 * This file reads the `budget` object of `GET /v1/accounts/{address}`, builds
 * the two actions and words the errors. Every field is read as optional: a
 * node older than the lock reports fewer of them.
 */
import type { Action } from '../inpage/types';
import { formatAmount, formatDuration } from './format';

export const ORCAS_PER_KEEL = 1_000_000n;
/** `budget.free_lock_min_keel`: a lock of at least this much is accepted from an account with no capacity. */
export const FIRST_LOCK_MIN_ORCAS = ORCAS_PER_KEEL;
/** Actions per day one locked KEEL grants, used in the copy when the node does not say. */
export const ACTIONS_PER_KEEL_PER_DAY = 100;

export interface PendingUnlock {
  /** When the KEEL is back in the spendable balance (unix ms). */
  readyAt: number;
  /** Orcas. */
  amount: bigint;
}

export interface Capacity {
  /** Actions the account can send now. */
  available: number;
  /** KEEL locked, in orcas. */
  locked: bigint;
  /** Actions per day the lock grants. */
  perDay: number;
  /** Unlocks on their way back, soonest first. */
  unlocking: PendingUnlock[];
  /**
   * Actions left today that the plan of the client (the exchange that
   * onboarded this account) pays for, once the account's own are used up.
   */
  sponsored: number;
}

export const NO_CAPACITY: Capacity = { available: 0, locked: 0n, perDay: 0, unlocking: [], sponsored: 0 };

function count(v: unknown): number | null {
  const n = typeof v === 'number' ? v : typeof v === 'string' && /^\d+$/.test(v.trim()) ? Number(v.trim()) : NaN;
  return Number.isFinite(n) && n >= 0 ? Math.floor(n) : null;
}

function orcas(v: unknown): bigint | null {
  if (typeof v === 'bigint') return v >= 0n ? v : null;
  if (typeof v === 'number') return Number.isSafeInteger(v) && v >= 0 ? BigInt(v) : null;
  if (typeof v === 'string' && /^\d+$/.test(v.trim())) return BigInt(v.trim());
  return null;
}

function unlocks(v: unknown): PendingUnlock[] {
  if (!Array.isArray(v)) return [];
  const out: PendingUnlock[] = [];
  for (const row of v) {
    if (typeof row !== 'object' || row === null) continue;
    const r = row as Record<string, unknown>;
    const readyAt = count(r['ready_at']);
    const amount = orcas(r['amount']);
    if (readyAt !== null && amount !== null && amount > 0n) out.push({ readyAt, amount });
  }
  return out.sort((a, b) => a.readyAt - b.readyAt);
}

function sponsoredLeft(v: unknown): number {
  if (typeof v !== 'object' || v === null) return 0;
  return count((v as Record<string, unknown>)['left_today']) ?? 0;
}

/**
 * The capacity in an account payload, or null when the payload carries no
 * `budget` object. `remaining` counts the lock pool as of the account's last
 * action, while `lock_pool` is refilled to the current block, so the two are
 * combined from their parts when the node reports them.
 */
export function parseCapacity(payload: unknown): Capacity | null {
  if (typeof payload !== 'object' || payload === null) return null;
  const p = payload as Record<string, unknown>;
  const raw = p['budget'];
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return null;
  const b = raw as Record<string, unknown>;
  const remaining = count(b['remaining']);
  const limit = count(b['limit']);
  const used = count(b['used']);
  const pool = count(b['lock_pool']);
  const fromParts = limit !== null && used !== null && pool !== null ? Math.max(0, limit - used) + pool : null;
  const pending = unlocks(b['unlocking']);
  return {
    available: Math.max(remaining ?? 0, fromParts ?? 0),
    locked: orcas(b['locked']) ?? 0n,
    perDay: count(b['lock_cap_per_day']) ?? 0,
    unlocking: pending.length > 0 ? pending : unlocks(p['unlocking']),
    sponsored: sponsoredLeft(b['sponsor']),
  };
}

/** True when the account cannot act and holds no lock: the card then explains the first lock. */
export function needsFirstLock(c: Capacity): boolean {
  return c.available === 0 && c.locked === 0n && c.sponsored === 0;
}

export function lockBudgetAction(amount: bigint): Action {
  return { LockBudget: { amount } };
}

export function unlockBudgetAction(amount: bigint): Action {
  return { UnlockBudget: { amount } };
}

/**
 * Why this lock or unlock cannot be sent, or null when it can. `spendable` is
 * the KEEL in the available balance, in orcas.
 */
export function capacityChangeProblem(kind: 'lock' | 'unlock', amount: bigint | null, c: Capacity, spendable: bigint): string | null {
  if (amount === null || amount <= 0n) return 'Enter an amount in KEEL, with at most 6 decimals.';
  if (kind === 'lock') {
    if (amount > spendable) return `More than the ${formatAmount(spendable, 6)} KEEL available.`;
    if (c.available === 0 && c.sponsored === 0 && amount < FIRST_LOCK_MIN_ORCAS) return `With no capacity left, a lock must be at least ${formatAmount(FIRST_LOCK_MIN_ORCAS, 6)} KEEL.`;
    return null;
  }
  if (c.locked === 0n) return 'Nothing is locked.';
  if (amount > c.locked) return `More than the ${formatAmount(c.locked, 6)} KEEL locked.`;
  if (c.available === 0 && c.sponsored === 0) return 'Unlocking is an action too, and none are left right now. The capacity refills over the day.';
  return null;
}

/** When a pending unlock arrives, relative to `nowMs`: "in 3 d", or "arriving now" once it is due. */
export function unlockArrival(readyAtMs: number, nowMs: number): string {
  const secs = Math.ceil((readyAtMs - nowMs) / 1000);
  return secs <= 0 ? 'arriving now' : `in ${formatDuration(secs)}`;
}

export const NO_CAPACITY_MESSAGE = `This account has no action capacity. Lock 1 KEEL on Home to get ${ACTIONS_PER_KEEL_PER_DAY} actions a day.`;

/** The text for a failed action: the node's error `code` in words where the wallet knows the remedy, else its message. */
export function actionErrorText(code: unknown, message: unknown): string {
  if (code === 'BUDGET_EXHAUSTED') return NO_CAPACITY_MESSAGE;
  return typeof message === 'string' && message.length > 0 ? message : 'rejected';
}
