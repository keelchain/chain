/**
 * Wire protocol shared by the inpage provider, the content script bridge,
 * the background service worker and the popup. Everything that crosses a
 * process boundary is JSON (chrome.runtime messages are JSON-serialised),
 * so bigints travel as `{ "$bigint": "123" }` markers (see `toWire`/`fromWire`).
 */
import type { ErrorCode, ProviderError, ProviderEventName } from '../inpage/types';

export const CHANNEL = 'keel-wallet-v1';
export const PROVIDER_VERSION = '1.0.0';

export const ERROR_CODES = ['USER_REJECTED', 'LOCKED', 'NO_ACCOUNT', 'NOT_CONNECTED', 'INVALID_REQUEST', 'WRONG_NETWORK'] as const;

export class WalletError extends Error {
  readonly code: ErrorCode;
  constructor(code: ErrorCode, message?: string) {
    super(message ?? DEFAULT_MESSAGES[code]);
    this.name = 'WalletError';
    this.code = code;
  }
  toJSON(): ProviderError {
    return { code: this.code, message: this.message };
  }
}

const DEFAULT_MESSAGES: Record<ErrorCode, string> = {
  USER_REJECTED: 'The user rejected the request.',
  LOCKED: 'The wallet is locked.',
  NO_ACCOUNT: 'The wallet has no account.',
  NOT_CONNECTED: 'This site is not connected to the wallet.',
  INVALID_REQUEST: 'Invalid request.',
  WRONG_NETWORK: 'The wallet is on a different network.',
};

export function isErrorCode(v: unknown): v is ErrorCode {
  return typeof v === 'string' && (ERROR_CODES as readonly string[]).includes(v);
}

/** Turns anything thrown inside the wallet into a `{code, message}` the page can handle. */
export function toProviderError(e: unknown): ProviderError {
  if (e instanceof WalletError) return e.toJSON();
  if (typeof e === 'object' && e !== null && isErrorCode((e as { code?: unknown }).code)) {
    const m = (e as { message?: unknown }).message;
    return { code: (e as { code: ErrorCode }).code, message: typeof m === 'string' ? m : DEFAULT_MESSAGES[(e as { code: ErrorCode }).code] };
  }
  return { code: 'INVALID_REQUEST', message: e instanceof Error ? e.message : String(e) };
}

export const PROVIDER_METHODS = ['connect', 'disconnect', 'getAccount', 'signMessage', 'signAction', 'authorizeSession'] as const;
export type ProviderMethod = (typeof PROVIDER_METHODS)[number];

export function isProviderMethod(v: unknown): v is ProviderMethod {
  return typeof v === 'string' && (PROVIDER_METHODS as readonly string[]).includes(v);
}

/** Page → wallet request. `params` is a JSON value (bigints already marked). */
export interface ProviderRequest {
  id: string;
  method: ProviderMethod;
  params: unknown;
}

export type ProviderResponse =
  | { id: string; ok: true; result: unknown }
  | { id: string; ok: false; error: ProviderError };

export interface ProviderEvent {
  event: ProviderEventName;
  payload: unknown;
}

// ------------------------------------------------------------ window.postMessage envelopes

export interface InpageToContent {
  channel: typeof CHANNEL;
  dir: 'to-wallet';
  request: ProviderRequest;
}

export type ContentToInpage =
  | { channel: typeof CHANNEL; dir: 'to-page'; response: ProviderResponse }
  | { channel: typeof CHANNEL; dir: 'to-page'; event: ProviderEvent };

export function isInpageToContent(v: unknown): v is InpageToContent {
  if (typeof v !== 'object' || v === null) return false;
  const m = v as Record<string, unknown>;
  if (m['channel'] !== CHANNEL || m['dir'] !== 'to-wallet') return false;
  const r = m['request'];
  return typeof r === 'object' && r !== null && typeof (r as ProviderRequest).id === 'string' && isProviderMethod((r as ProviderRequest).method);
}

export function isContentToInpage(v: unknown): v is ContentToInpage {
  if (typeof v !== 'object' || v === null) return false;
  const m = v as Record<string, unknown>;
  return m['channel'] === CHANNEL && m['dir'] === 'to-page' && ('response' in m || 'event' in m);
}

// ------------------------------------------------------------ chrome.runtime messages

/** Content script → background: a page request. The origin is taken from the sender, never from the payload. */
export interface RuntimeProviderMessage {
  kind: 'keel:provider';
  request: ProviderRequest;
}

/** Background → content script: an event to forward to the page. */
export interface RuntimeEventMessage {
  kind: 'keel:event';
  event: ProviderEvent;
}

/** Popup → background. */
export interface RuntimeUiMessage {
  kind: 'keel:ui';
  method: string;
  params: unknown;
}

export type RuntimeMessage = RuntimeProviderMessage | RuntimeEventMessage | RuntimeUiMessage;

export function isRuntimeMessage(v: unknown): v is RuntimeMessage {
  if (typeof v !== 'object' || v === null) return false;
  const k = (v as { kind?: unknown }).kind;
  return k === 'keel:provider' || k === 'keel:event' || k === 'keel:ui';
}

// ------------------------------------------------------------ bigint-safe JSON

const BIG = '$bigint';

/** Replaces bigints with `{ "$bigint": "…" }` markers so the value survives JSON serialisation. */
export function toWire(v: unknown): unknown {
  if (typeof v === 'bigint') return { [BIG]: v.toString() };
  if (Array.isArray(v)) return v.map(toWire);
  if (v instanceof Uint8Array) return Array.from(v);
  if (typeof v === 'object' && v !== null) {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) if (x !== undefined) out[k] = toWire(x);
    return out;
  }
  if (typeof v === 'function' || typeof v === 'symbol') return undefined;
  return v;
}

/** Inverse of `toWire`. */
export function fromWire(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(fromWire);
  if (typeof v === 'object' && v !== null) {
    const rec = v as Record<string, unknown>;
    const keys = Object.keys(rec);
    if (keys.length === 1 && keys[0] === BIG && typeof rec[BIG] === 'string' && /^-?\d+$/.test(rec[BIG] as string)) return BigInt(rec[BIG] as string);
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(rec)) out[k] = fromWire(x);
    return out;
  }
  return v;
}

let counter = 0;
export function newId(prefix = 'req'): string {
  counter += 1;
  const rnd = Math.floor(Math.random() * 0xffffffff).toString(16).padStart(8, '0');
  return `${prefix}-${Date.now().toString(36)}-${counter}-${rnd}`;
}
