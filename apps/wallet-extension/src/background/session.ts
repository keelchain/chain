/**
 * Unlocked secrets live only here, in service-worker memory: the 64-byte
 * BIP39 seed and a cache of derived keypairs. An inactivity timer wipes them.
 * If the browser evicts the worker the memory goes with it, which is the
 * same as locking.
 */
import type { Keypair } from '@keelchain/sdk';
import { accountKeypair } from '../core/keys';

export interface Timer {
  set(ms: number, fn: () => void): unknown;
  clear(handle: unknown): void;
}

const defaultTimer: Timer = {
  set: (ms, fn) => setTimeout(fn, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
};

export class Session {
  private seed: Uint8Array | null = null;
  private keys = new Map<number, Keypair>();
  private handle: unknown = null;
  private lockAtMs = 0;
  private ttlMs = 15 * 60_000;
  onLock: (() => void) | null = null;

  constructor(private readonly timer: Timer = defaultTimer, private readonly now: () => number = () => Date.now()) {}

  get unlocked(): boolean {
    if (this.seed !== null && this.now() >= this.lockAtMs) this.lock();
    return this.seed !== null;
  }

  /** Milliseconds until auto-lock, 0 when locked. */
  get remainingMs(): number {
    return this.unlocked ? Math.max(0, this.lockAtMs - this.now()) : 0;
  }

  unlock(seed: Uint8Array, ttlMinutes: number): void {
    this.wipe();
    this.seed = seed.slice();
    this.ttlMs = Math.max(1, ttlMinutes) * 60_000;
    this.touch();
  }

  /** Any user activity restarts the inactivity timer. */
  touch(): void {
    if (this.seed === null) return;
    this.lockAtMs = this.now() + this.ttlMs;
    if (this.handle !== null) this.timer.clear(this.handle);
    this.handle = this.timer.set(this.ttlMs, () => this.lock());
  }

  setTtl(ttlMinutes: number): void {
    this.ttlMs = Math.max(1, ttlMinutes) * 60_000;
    this.touch();
  }

  lock(): void {
    const was = this.seed !== null;
    this.wipe();
    if (was) this.onLock?.();
  }

  keypair(index: number): Keypair | null {
    if (!this.unlocked || this.seed === null) return null;
    let k = this.keys.get(index);
    if (!k) {
      k = accountKeypair(this.seed, index);
      this.keys.set(index, k);
    }
    return k;
  }

  /** The raw seed, for deriving a new account's address. */
  currentSeed(): Uint8Array | null {
    return this.unlocked ? this.seed : null;
  }

  private wipe(): void {
    if (this.seed) this.seed.fill(0);
    for (const k of this.keys.values()) k.secret.fill(0);
    this.keys.clear();
    this.seed = null;
    this.lockAtMs = 0;
    if (this.handle !== null) this.timer.clear(this.handle);
    this.handle = null;
  }
}
