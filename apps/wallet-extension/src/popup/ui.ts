/** Popup ↔ background client. */
import { fromWire, toWire, type RuntimeUiMessage } from '../core/protocol';
import type { UiState } from '../background/wallet';

export type { UiState };

export async function call<T = unknown>(method: string, params: unknown = {}): Promise<T> {
  const msg: RuntimeUiMessage = { kind: 'keel:ui', method, params: toWire(params) };
  const res = (await chrome.runtime.sendMessage(msg)) as { ok: true; result: unknown } | { ok: false; error: string } | undefined;
  if (!res) throw new Error('The wallet background is not responding.');
  if (!res.ok) throw new Error(res.error);
  return fromWire(res.result) as T;
}

export const getState = () => call<UiState>('getState');

export interface Balance {
  asset: string;
  account_type: string;
  balance: string | number;
}

export interface AccountResponse {
  nonce: number;
  balances: Balance[];
  tier?: number;
}

/** GET <rpc>/v1/accounts/<addr>; an unknown account (404) is an empty one. */
export async function fetchAccount(rpc: string, address: string): Promise<AccountResponse> {
  const res = await fetch(`${rpc.replace(/\/$/, '')}/v1/accounts/${address}`);
  if (res.status === 404) return { nonce: 0, balances: [] };
  if (!res.ok) throw new Error(`RPC ${res.status}`);
  const j = (await res.json()) as Partial<AccountResponse>;
  return { nonce: Number(j.nonce ?? 0), balances: Array.isArray(j.balances) ? j.balances : [], ...(j.tier !== undefined ? { tier: j.tier } : {}) };
}
