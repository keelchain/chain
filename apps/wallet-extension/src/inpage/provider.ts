/**
 * The `window.keel` provider (also exposed as `window.stt`). It runs in the page's main world and talks to
 * the content script over a `Transport` (window.postMessage in the browser,
 * an in-memory pair in tests). Nothing here touches keys.
 */
import {
  CHANNEL,
  PROVIDER_VERSION,
  fromWire,
  isContentToInpage,
  newId,
  toWire,
  type ContentToInpage,
  type InpageToContent,
  type ProviderMethod,
  type ProviderResponse,
} from '../core/protocol';
import type { AccountInfo, AuthorizeSessionRequest, ErrorCode, ProviderError, ProviderEventName, SignActionRequest, SignActionResult, SttProvider } from './types';

export interface Transport {
  send(msg: InpageToContent): void;
  /** Subscribe to wallet → page messages. */
  onMessage(cb: (msg: ContentToInpage) => void): void;
}

/** Rejection value: an Error that is also a `{ code, message }`. */
export class SttProviderError extends Error implements ProviderError {
  readonly code: ErrorCode;
  constructor(code: ErrorCode, message: string) {
    super(message);
    this.name = 'SttProviderError';
    this.code = code;
  }
}

interface PendingCall {
  resolve: (v: unknown) => void;
  reject: (e: SttProviderError) => void;
}

export function createProvider(transport: Transport): SttProvider {
  const pending = new Map<string, PendingCall>();
  const listeners = new Map<ProviderEventName, Set<(payload: unknown) => void>>();

  transport.onMessage((msg) => {
    if ('response' in msg) {
      const r: ProviderResponse = msg.response;
      const p = pending.get(r.id);
      if (!p) return;
      pending.delete(r.id);
      if (r.ok) p.resolve(fromWire(r.result));
      else p.reject(new SttProviderError(r.error.code, r.error.message));
    } else if ('event' in msg) {
      const set = listeners.get(msg.event.event);
      if (!set) return;
      const payload = fromWire(msg.event.payload);
      for (const h of Array.from(set)) {
        try {
          h(payload);
        } catch {
          /* a listener's error must not break the others */
        }
      }
    }
  });

  function call<T>(method: ProviderMethod, params?: unknown): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      let wire: unknown;
      try {
        wire = params === undefined ? null : toWire(params);
      } catch (e) {
        reject(new SttProviderError('INVALID_REQUEST', e instanceof Error ? e.message : String(e)));
        return;
      }
      const id = newId('keel');
      pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      transport.send({ channel: CHANNEL, dir: 'to-wallet', request: { id, method, params: wire } });
    });
  }

  const provider: SttProvider = {
    isKeel: true,
    isStt: true,
    version: PROVIDER_VERSION,
    connect: (opts) => call<AccountInfo>('connect', opts ?? {}),
    disconnect: async () => {
      await call<null>('disconnect');
    },
    getAccount: () => call<AccountInfo | null>('getAccount'),
    signMessage: (message) => call<{ address: string; signature: string }>('signMessage', message),
    signAction: (req: SignActionRequest) => call<SignActionResult>('signAction', req),
    authorizeSession: (req: AuthorizeSessionRequest) => call<SignActionResult>('authorizeSession', req),
    on: (event, handler) => {
      let set = listeners.get(event);
      if (!set) {
        set = new Set();
        listeners.set(event, set);
      }
      set.add(handler);
      return () => {
        listeners.get(event)?.delete(handler);
      };
    },
  };
  return Object.freeze(provider);
}

/** Transport over `window.postMessage`, used by the injected script. */
export function windowTransport(win: Window): Transport {
  return {
    send: (msg) => win.postMessage(msg, '*'),
    onMessage: (cb) => {
      win.addEventListener('message', (ev: MessageEvent<unknown>) => {
        if (ev.source !== win) return;
        if (isContentToInpage(ev.data)) cb(ev.data);
      });
    },
  };
}
