/**
 * Network list from `VITE_NETWORKS` (JSON array of {id, name, api}). The
 * selected network is persisted in localStorage and mirrored in the URL as
 * the first path segment (`/testnet/...`).
 */
export interface Network {
  id: string;
  name: string;
  api: string;
}

export const NETWORK_STORAGE_KEY = 'keel-explorer-network';

export const DEFAULT_NETWORKS: Network[] = [
  { id: 'testnet', name: 'Testnet', api: 'http://127.0.0.1:6100' },
  { id: 'mainnet', name: 'Mainnet', api: 'http://127.0.0.1:6101' },
];

export function parseNetworks(raw: string | undefined): Network[] {
  if (!raw) return DEFAULT_NETWORKS;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return DEFAULT_NETWORKS;
    const out: Network[] = [];
    for (const item of parsed) {
      if (typeof item !== 'object' || item === null) continue;
      const { id, name, api } = item as Record<string, unknown>;
      if (typeof id !== 'string' || !/^[a-z0-9-]+$/.test(id)) continue;
      if (typeof api !== 'string') continue;
      out.push({ id, name: typeof name === 'string' ? name : id, api });
    }
    return out.length ? out : DEFAULT_NETWORKS;
  } catch {
    return DEFAULT_NETWORKS;
  }
}

export const NETWORKS: Network[] = parseNetworks(import.meta.env.VITE_NETWORKS);

export function isMockEnabled(): boolean {
  const v = import.meta.env.VITE_API_MOCK;
  return v === '1' || v === 'true';
}

export function readStoredNetwork(list: Network[] = NETWORKS): Network {
  try {
    const id = window.localStorage.getItem(NETWORK_STORAGE_KEY);
    const found = list.find((n) => n.id === id);
    if (found) return found;
  } catch {
    // storage unavailable
  }
  return list[0]!;
}

export function storeNetwork(id: string): void {
  try {
    window.localStorage.setItem(NETWORK_STORAGE_KEY, id);
  } catch {
    // best effort
  }
}
