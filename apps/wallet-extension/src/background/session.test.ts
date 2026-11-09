import { describe, expect, it } from 'vitest';
import { Session, type SessionStore, type Timer } from './session';

class FakeTimer implements Timer {
  fns: (() => void)[] = [];
  set(_ms: number, fn: () => void): unknown {
    this.fns.push(fn);
    return fn;
  }
  clear(handle: unknown): void {
    this.fns = this.fns.filter((f) => f !== handle);
  }
}

function memoryStore(): SessionStore & { value: { seedHex: string; lockAtMs: number } | null } {
  return {
    value: null,
    async get() {
      return this.value;
    },
    async set(v) {
      this.value = v;
    },
    async remove() {
      this.value = null;
    },
  };
}

const SEED = new Uint8Array(64).map((_, i) => i);

describe('session persistence across worker restarts', () => {
  it('a new worker restores an unlocked session whose deadline has not passed', async () => {
    let now = 1_000_000;
    const store = memoryStore();
    const first = new Session(new FakeTimer(), () => now, store);
    first.unlock(SEED, 15);
    await Promise.resolve();
    expect(store.value?.lockAtMs).toBe(now + 15 * 60_000);
    expect(store.value?.seedHex.length).toBe(128);

    // The worker dies; a fresh Session with the same store comes up 5 minutes later.
    now += 5 * 60_000;
    const second = new Session(new FakeTimer(), () => now, store);
    expect(second.unlocked).toBe(false);
    expect(await second.restore()).toBe(true);
    expect(second.unlocked).toBe(true);
    expect(second.remainingMs).toBe(10 * 60_000);
    expect(second.keypair(0)?.address).toBe(first.keypair(0)?.address);
  });

  it('an expired mirror is discarded, and locking removes it', async () => {
    let now = 1_000_000;
    const store = memoryStore();
    const s = new Session(new FakeTimer(), () => now, store);
    s.unlock(SEED, 1);
    await Promise.resolve();
    now += 2 * 60_000;
    const later = new Session(new FakeTimer(), () => now, store);
    expect(await later.restore()).toBe(false);
    expect(later.unlocked).toBe(false);
    expect(store.value).toBeNull();

    const t = new Session(new FakeTimer(), () => now, store);
    t.unlock(SEED, 15);
    await Promise.resolve();
    expect(store.value).not.toBeNull();
    t.lock();
    await Promise.resolve();
    expect(store.value).toBeNull();
  });

  it('touching extends the mirrored deadline', async () => {
    let now = 1_000_000;
    const store = memoryStore();
    const s = new Session(new FakeTimer(), () => now, store);
    s.unlock(SEED, 15);
    now += 10 * 60_000;
    s.touch();
    await Promise.resolve();
    expect(store.value?.lockAtMs).toBe(now + 15 * 60_000);
  });
});
