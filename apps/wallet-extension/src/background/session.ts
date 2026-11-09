/**
 * Unlocked secrets live here: the 64-byte BIP39 seed and a cache of derived
 * keypairs. An inactivity timer wipes them.
 *
 * A Manifest V3 service worker is evicted after about thirty seconds without
 * work, and its memory goes with it. Left at that, the wallet would ask for
 * the password every time the popup reopened, whatever the auto-lock says.
 * So the seed and its lock deadline are mirrored into an optional
 * `SessionStore` (`chrome.storage.session`: memory only, extension contexts
 * only, gone when the browser closes), and a freshly started worker restores
 * them when the deadline has not passed. Locking removes the mirror.
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

/** Where the unlocked seed is mirrored across worker restarts. */
export interface SessionStore {
  get(): Promise<{ seedHex: string; lockAtMs: number } | null>;
  set(v: { seedHex: string; lockAtMs: number }): Promise<void>;
  remove(): Promise<void>;
}

export class Session {
  private seed: Uint8Array | null = null;
  private keys = new Map<number, Keypair>();
  private handle: unknown = null;
  private lockAtMs = 0;
  private ttlMs = 15 * 60_000;
  onLock: (() => void) | null = null;

  constructor(
    private readonly timer: Timer = defaultTimer,
    private readonly now: () => number = () => Date.now(),
    private readonly store: SessionStore | null = null,
  ) {}

  /** Restores an unlocked session mirrored by a previous worker, when its deadline has not passed. */
  async restore(): Promise<boolean> {
    if (!this.store || this.seed !== null) return false;
    const saved = await this.store.get().catch(() => null);
    if (!saved || !/^[0-9a-f]{128}$/.test(saved.seedHex) || saved.lockAtMs <= this.now()) {
      if (saved) await this.store.remove().catch(() => undefined);
      return false;
    }
    this.seed = Uint8Array.from(saved.seedHex.match(/../g)!.map((b) => parseInt(b, 16)));
    this.lockAtMs = saved.lockAtMs;
    if (this.handle !== null) this.timer.clear(this.handle);
    this.handle = this.timer.set(Math.max(1, this.lockAtMs - this.now()), () => this.lock());
    return true;
  }

  private persist(): void {
    if (!this.store) return;
    if (this.seed === null) {
      void this.store.remove().catch(() => undefined);
      return;
    }
    const seedHex = Array.from(this.seed, (b) => b.toString(16).padStart(2, '0')).join('');
    void this.store.set({ seedHex, lockAtMs: this.lockAtMs }).catch(() => undefined);
  }

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
    this.persist();
  }

  setTtl(ttlMinutes: number): void {
    this.ttlMs = Math.max(1, ttlMinutes) * 60_000;
    this.touch();
  }

  lock(): void {
    const was = this.seed !== null;
    this.wipe();
    this.persist();
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
