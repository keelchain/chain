/**
 * Action helpers on top of `@keelchain/sdk`: session-key variants (chain-side
 * work in progress), scope bits, and validation of what a page hands us.
 */
import { encodeAction, encodeEnvelope, SESSION_SCOPE, type Action as SdkAction } from '@keelchain/sdk';
import type { Action, Envelope, SessionScope } from '../inpage/types';
import { SESSION_SCOPE_BITS } from '../inpage/types';
import { WalletError } from './protocol';

export const SCOPE_MARKETS: number = SESSION_SCOPE.MARKETS;
export const SCOPE_P2P_MANAGE: number = SESSION_SCOPE.P2P_MANAGE;
export const SESSION_SCOPES: readonly SessionScope[] = ['markets', 'p2p_manage'];
/** `AuthorizeSessionKey.expires_at` may be at most this far ahead (docs/wallet.md §4). */
export const MAX_SESSION_SECS = 30 * 86400;

export function scopeBits(scopes: readonly SessionScope[]): number {
  return scopes.reduce((bits, s) => bits | SESSION_SCOPE_BITS[s], 0);
}

export function scopesFromBits(bits: number): SessionScope[] {
  return SESSION_SCOPES.filter((s) => (bits & SESSION_SCOPE_BITS[s]) !== 0);
}

export function isSessionScope(v: unknown): v is SessionScope {
  return typeof v === 'string' && (SESSION_SCOPES as readonly string[]).includes(v);
}

export function isHex(v: unknown, bytes?: number): v is string {
  if (typeof v !== 'string') return false;
  const h = v.startsWith('0x') ? v.slice(2) : v;
  if (!/^[0-9a-fA-F]*$/.test(h) || h.length % 2 !== 0) return false;
  return bytes === undefined || h.length === bytes * 2;
}

export function normalizeHex(v: string): string {
  return (v.startsWith('0x') ? v.slice(2) : v).toLowerCase();
}

/** Splits an externally tagged enum value into [variant, payload]. */
export function untag(value: unknown): [string, Record<string, unknown>] {
  if (typeof value === 'string') return [value, {}];
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    const keys = Object.keys(value);
    if (keys.length === 1) {
      const k = keys[0]!;
      const payload = (value as Record<string, unknown>)[k];
      if (typeof payload === 'object' && payload !== null && !Array.isArray(payload)) return [k, payload as Record<string, unknown>];
      return [k, { value: payload }];
    }
  }
  return ['Unknown', { value }];
}

export function actionKind(action: unknown): string {
  return untag(action)[0];
}

/** Borsh-encodes an action with the SDK (session-key variants included); failures become INVALID_REQUEST. */
export function encodeWalletAction(action: Action): Uint8Array {
  const kind = actionKind(action);
  try {
    return encodeAction(action as unknown as SdkAction).bytes();
  } catch (e) {
    throw new WalletError('INVALID_REQUEST', `Cannot encode action ${kind}: ${e instanceof Error ? e.message : String(e)}`);
  }
}

function isSafeInt(v: unknown): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
}

/** Validates the envelope a page sent and returns it normalised (signer lowercase hex). */
export function validateEnvelope(raw: unknown): Envelope {
  if (typeof raw !== 'object' || raw === null) throw new WalletError('INVALID_REQUEST', 'envelope must be an object');
  const e = raw as Record<string, unknown>;
  if (!isHex(e['signer'], 32)) throw new WalletError('INVALID_REQUEST', 'envelope.signer must be a 32-byte hex address');
  if (!isSafeInt(e['nonce'])) throw new WalletError('INVALID_REQUEST', 'envelope.nonce must be a non-negative integer');
  if (!isSafeInt(e['chain_id']) || e['chain_id'] > 0xffffffff) throw new WalletError('INVALID_REQUEST', 'envelope.chain_id must be a u32');
  const action = e['action'];
  const [kind] = untag(action);
  if (kind === 'Unknown') throw new WalletError('INVALID_REQUEST', 'envelope.action must be an externally tagged enum value');
  const envelope: Envelope = { signer: normalizeHex(e['signer']), nonce: e['nonce'], chain_id: e['chain_id'], action: action as Action };
  // Encode once up front so the user never approves something that cannot be signed.
  try {
    encodeEnvelope({ signer: envelope.signer, nonce: envelope.nonce, chain_id: envelope.chain_id, action: action as unknown as SdkAction });
  } catch (err) {
    if (err instanceof WalletError) throw err;
    throw new WalletError('INVALID_REQUEST', `Cannot encode action ${kind}: ${err instanceof Error ? err.message : String(err)}`);
  }
  return envelope;
}

export interface SessionRequest {
  key: string;
  scope: SessionScope[];
  expires_at: number;
  nonce: number;
  chain_id: number;
}

export function validateSessionRequest(raw: unknown, nowSecs = Math.floor(Date.now() / 1000)): SessionRequest {
  if (typeof raw !== 'object' || raw === null) throw new WalletError('INVALID_REQUEST', 'request must be an object');
  const r = raw as Record<string, unknown>;
  if (!isHex(r['key'], 32)) throw new WalletError('INVALID_REQUEST', 'key must be a 32-byte hex public key');
  const scope = r['scope'];
  if (!Array.isArray(scope) || scope.length === 0 || !scope.every(isSessionScope)) {
    throw new WalletError('INVALID_REQUEST', `scope must be a non-empty array of ${SESSION_SCOPES.join(' | ')}`);
  }
  if (!isSafeInt(r['expires_at'])) throw new WalletError('INVALID_REQUEST', 'expires_at must be unix seconds');
  if (r['expires_at'] <= nowSecs) throw new WalletError('INVALID_REQUEST', 'expires_at is in the past');
  if (r['expires_at'] > nowSecs + MAX_SESSION_SECS) throw new WalletError('INVALID_REQUEST', 'expires_at may be at most 30 days ahead');
  if (!isSafeInt(r['nonce'])) throw new WalletError('INVALID_REQUEST', 'nonce must be a non-negative integer');
  if (!isSafeInt(r['chain_id'])) throw new WalletError('INVALID_REQUEST', 'chain_id must be a u32');
  const uniq = Array.from(new Set(scope as SessionScope[]));
  return { key: normalizeHex(r['key']), scope: uniq, expires_at: r['expires_at'], nonce: r['nonce'], chain_id: r['chain_id'] };
}

export function sessionAction(req: SessionRequest): Action {
  return { AuthorizeSessionKey: { key: req.key, scope: scopeBits(req.scope), expires_at: req.expires_at } };
}
