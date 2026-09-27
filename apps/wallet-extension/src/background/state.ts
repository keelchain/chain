/** Persisted (non-secret) wallet state. Secrets live only in the encrypted vault blob. */
import type { EncryptedVault } from '../core/vault';
import { DEFAULT_NETWORK_ID, type Network } from '../core/networks';
import type { KeyValueStore } from '../core/storage';

export interface AccountMeta {
  index: number;
  address: string;
  name: string;
}

export interface Connection {
  connectedAt: number;
}

export interface WalletState {
  vault: EncryptedVault | null;
  accounts: AccountMeta[];
  activeIndex: number;
  networkId: string;
  /** networkId → origin → connection. */
  connections: Record<string, Record<string, Connection>>;
  settings: { autoLockMinutes: number };
  /** Networks the user added (RPC + chain id), on top of the built-in ones. */
  customNetworks: Network[];
}

export const DEFAULT_AUTO_LOCK_MINUTES = 15;
const KEY = 'wallet';

export function emptyState(): WalletState {
  return { vault: null, accounts: [], activeIndex: 0, networkId: DEFAULT_NETWORK_ID, connections: {}, settings: { autoLockMinutes: DEFAULT_AUTO_LOCK_MINUTES }, customNetworks: [] };
}

export async function loadState(store: KeyValueStore): Promise<WalletState> {
  const s = await store.get<Partial<WalletState>>(KEY);
  const d = emptyState();
  if (!s) return d;
  return {
    vault: s.vault ?? null,
    accounts: Array.isArray(s.accounts) ? s.accounts : [],
    activeIndex: typeof s.activeIndex === 'number' ? s.activeIndex : 0,
    networkId: typeof s.networkId === 'string' ? s.networkId : d.networkId,
    connections: typeof s.connections === 'object' && s.connections !== null ? s.connections : {},
    settings: { autoLockMinutes: s.settings?.autoLockMinutes ?? DEFAULT_AUTO_LOCK_MINUTES },
    customNetworks: Array.isArray(s.customNetworks) ? s.customNetworks.filter(isNetwork) : [],
  };
}

export async function saveState(store: KeyValueStore, state: WalletState): Promise<void> {
  await store.set(KEY, state);
}

function isNetwork(n: unknown): n is Network {
  if (typeof n !== 'object' || n === null) return false;
  const o = n as Record<string, unknown>;
  return typeof o['id'] === 'string' && typeof o['name'] === 'string' && typeof o['rpc'] === 'string' && typeof o['chainId'] === 'number' && typeof o['explorer'] === 'string';
}
