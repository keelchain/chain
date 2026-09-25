/** Minimal key/value store abstraction over chrome.storage.local (memory-backed in tests). */
export interface KeyValueStore {
  get<T>(key: string): Promise<T | undefined>;
  set<T>(key: string, value: T): Promise<void>;
  remove(key: string): Promise<void>;
}

export class MemoryStore implements KeyValueStore {
  private readonly map = new Map<string, unknown>();
  async get<T>(key: string): Promise<T | undefined> {
    const v = this.map.get(key);
    return v === undefined ? undefined : (JSON.parse(JSON.stringify(v)) as T);
  }
  async set<T>(key: string, value: T): Promise<void> {
    this.map.set(key, JSON.parse(JSON.stringify(value)));
  }
  async remove(key: string): Promise<void> {
    this.map.delete(key);
  }
}

export class ChromeLocalStore implements KeyValueStore {
  async get<T>(key: string): Promise<T | undefined> {
    const r = await chrome.storage.local.get(key);
    return r[key] as T | undefined;
  }
  async set<T>(key: string, value: T): Promise<void> {
    await chrome.storage.local.set({ [key]: value });
  }
  async remove(key: string): Promise<void> {
    await chrome.storage.local.remove(key);
  }
}
