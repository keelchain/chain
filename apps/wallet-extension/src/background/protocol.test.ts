/**
 * End-to-end through the message protocol: a page-side provider built on an
 * in-memory transport, a fake content bridge and the real Wallet controller
 * (memory store, fake timer, scripted approvals).
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { Keypair, verify, txId, type SignedAction as SdkSignedAction } from '@keelchain/sdk';
import { createProvider, type Transport } from '../inpage/provider';
import { CHANNEL, toProviderError, type ContentToInpage, type InpageToContent, type ProviderEvent, type ProviderResponse } from '../core/protocol';
import { MemoryStore } from '../core/storage';
import { Approvals } from './approvals';
import { Session, type Timer } from './session';
import { Wallet } from './wallet';
import { VECTOR } from '../core/keys.test';
import type { KeelProvider, ProviderError } from '../inpage/types';

const ORIGIN = 'https://app.example';
const PASSWORD = 'correct horse battery';

class FakeTimer implements Timer {
  fns = new Map<number, () => void>();
  n = 0;
  set(_ms: number, fn: () => void) {
    this.fns.set(++this.n, fn);
    return this.n;
  }
  clear(h: unknown) {
    this.fns.delete(h as number);
  }
  fireAll() {
    for (const f of Array.from(this.fns.values())) f();
    this.fns.clear();
  }
}

interface Harness {
  provider: KeelProvider;
  wallet: Wallet;
  approvals: Approvals;
  session: Session;
  timer: FakeTimer;
  events: ProviderEvent[];
  /** Deliver a background event to the page, as the content script would. */
  emitted: Array<{ origin: string | null; event: ProviderEvent }>;
  sent: InpageToContent[];
  responses: ProviderResponse[];
}

let now = 1_800_000_000_000;

function harness(origin = ORIGIN): Harness {
  const approvals = new Approvals();
  const timer = new FakeTimer();
  const session = new Session(timer, () => now);
  const emitted: Harness['emitted'] = [];
  let pageListener: ((m: ContentToInpage) => void) | null = null;
  const wallet = new Wallet({
    store: new MemoryStore(),
    session,
    approvals,
    now: () => now,
    emit: (o, event) => {
      emitted.push({ origin: o, event });
      if (o === null || o === origin) pageListener?.({ channel: CHANNEL, dir: 'to-page', event });
    },
  });
  const sent: InpageToContent[] = [];
  const responses: ProviderResponse[] = [];
  // Fake content script: forward to the wallet with the trusted origin, answer with the same id.
  const transport: Transport = {
    send: (msg) => {
      sent.push(msg);
      wallet.handleProvider(origin, msg.request).then(
        (result) => {
          const response: ProviderResponse = { id: msg.request.id, ok: true, result };
          responses.push(response);
          pageListener?.({ channel: CHANNEL, dir: 'to-page', response });
        },
        (e: unknown) => {
          const response: ProviderResponse = { id: msg.request.id, ok: false, error: toProviderError(e) };
          responses.push(response);
          pageListener?.({ channel: CHANNEL, dir: 'to-page', response });
        },
      );
    },
    onMessage: (cb) => {
      pageListener = cb;
    },
  };
  const provider = createProvider(transport);
  const events: ProviderEvent[] = [];
  for (const name of ['accountChanged', 'disconnect', 'networkChanged'] as const) provider.on(name, (payload) => events.push({ event: name, payload }));
  return { provider, wallet, approvals, session, timer, events, emitted, sent, responses };
}

async function setup(h: Harness): Promise<void> {
  await h.wallet.createVault(VECTOR.phrase, PASSWORD);
}

/** Approve (or reject) the next pending request as the popup would. */
function autoDecide(h: Harness, approved: boolean) {
  h.approvals.onAdded = (req) => {
    void h.wallet.decideApproval(req.id, approved);
  };
}

async function rejection(p: Promise<unknown>): Promise<ProviderError> {
  try {
    await p;
  } catch (e) {
    const err = e as ProviderError;
    expect(typeof err.code).toBe('string');
    expect(typeof err.message).toBe('string');
    return { code: err.code, message: err.message };
  }
  throw new Error('expected rejection');
}

describe('provider protocol', () => {
  let h: Harness;
  beforeEach(() => {
    h = harness();
  });

  it('exposes the documented surface', () => {
    expect(h.provider.version).toBe('1.0.0');
    expect(Object.isFrozen(h.provider)).toBe(true);
  });

  it('matches responses to requests by id', async () => {
    await setup(h);
    autoDecide(h, true);
    const [a, b] = await Promise.all([h.provider.getAccount(), h.provider.connect()]);
    expect(a).toBeNull();
    expect(b).toEqual({ address: VECTOR.address0, network: 'testnet', chainId: 3 });
    expect(h.sent.map((m) => m.request.method)).toEqual(['getAccount', 'connect']);
    const ids = h.sent.map((m) => m.request.id);
    expect(new Set(ids).size).toBe(2);
    expect(h.responses.map((r) => r.id).sort()).toEqual([...ids].sort());
    expect(h.sent.every((m) => m.channel === CHANNEL && m.dir === 'to-wallet')).toBe(true);
  });

  it('NO_ACCOUNT before a vault exists; getAccount never prompts', async () => {
    let prompted = 0;
    h.approvals.onAdded = () => {
      prompted += 1;
    };
    expect((await rejection(h.provider.connect())).code).toBe('NO_ACCOUNT');
    expect(await h.provider.getAccount()).toBeNull();
    expect(prompted).toBe(0);
  });

  it('connect prompts once and is remembered per origin; disconnect emits', async () => {
    await setup(h);
    let prompted = 0;
    h.approvals.onAdded = (req) => {
      prompted += 1;
      expect(req.kind).toBe('connect');
      expect(req.origin).toBe(ORIGIN);
      void h.wallet.decideApproval(req.id, true);
    };
    await h.provider.connect();
    await h.provider.connect();
    expect(prompted).toBe(1);
    expect(await h.provider.getAccount()).toEqual({ address: VECTOR.address0, network: 'testnet', chainId: 3 });
    await h.provider.disconnect();
    expect(await h.provider.getAccount()).toBeNull();
    expect(h.events).toEqual([{ event: 'disconnect', payload: { network: 'testnet' } }]);
    const other = harness('https://other.example');
    expect(await other.provider.getAccount()).toBeNull();
  });

  it('USER_REJECTED when the user rejects', async () => {
    await setup(h);
    autoDecide(h, false);
    const err = await rejection(h.provider.connect());
    expect(err).toEqual({ code: 'USER_REJECTED', message: 'The user rejected the request.' });
  });

  it('NOT_CONNECTED for signing without a connection', async () => {
    await setup(h);
    expect((await rejection(h.provider.signMessage('hi'))).code).toBe('NOT_CONNECTED');
    expect((await rejection(h.provider.signAction({ envelope: { signer: VECTOR.address0, nonce: 0, chain_id: 3, action: 'ClaimRewards' } }))).code).toBe('NOT_CONNECTED');
    expect((await rejection(h.provider.authorizeSession({ key: VECTOR.address1, scope: ['markets'], expires_at: now / 1000 + 3600, nonce: 0, chain_id: 3 }))).code).toBe('NOT_CONNECTED');
  });

  it('LOCKED when the approval window is dismissed while locked, and after auto-lock', async () => {
    await setup(h);
    autoDecide(h, true);
    await h.provider.connect();
    h.session.lock();
    // The popup is closed without unlocking: the background rejects everything with LOCKED.
    h.approvals.onAdded = () => h.approvals.rejectAll(h.session.unlocked ? 'USER_REJECTED' : 'LOCKED');
    expect((await rejection(h.provider.signMessage('hi'))).code).toBe('LOCKED');
    // Approving while locked is refused by the UI layer.
    h.approvals.onAdded = (req) => {
      void h.wallet.decideApproval(req.id, true).catch(() => h.approvals.decide(req.id, false, 'LOCKED'));
    };
    expect((await rejection(h.provider.signMessage('hi'))).code).toBe('LOCKED');
    // Unlock, then let the inactivity timer fire: locked again.
    await h.wallet.unlock(PASSWORD);
    expect(h.session.unlocked).toBe(true);
    h.timer.fireAll();
    expect(h.session.unlocked).toBe(false);
    now += 16 * 60_000;
    await h.wallet.unlock(PASSWORD);
    now += 16 * 60_000;
    expect(h.session.unlocked).toBe(false);
  });

  it('signMessage shows the text and returns a verifiable signature', async () => {
    await setup(h);
    let shown = '';
    h.approvals.onAdded = (req) => {
      if (req.kind === 'signMessage') shown = req.message;
      void h.wallet.decideApproval(req.id, true);
    };
    await h.provider.connect();
    const msg = `Keel login\naddress: ${VECTOR.address0}\nnonce: 1`;
    const r = await h.provider.signMessage(msg);
    expect(shown).toBe(msg);
    expect(r.address).toBe(VECTOR.address0);
    expect(r.signature).toMatch(/^[0-9a-f]{128}$/);
    expect((await rejection(h.provider.signMessage(''))).code).toBe('INVALID_REQUEST');
  });

  it('signAction: decoded summary in the approval, SDK-verifiable result, bigints preserved', async () => {
    await setup(h);
    let summary = '';
    h.approvals.onAdded = (req) => {
      if (req.kind === 'signAction') {
        summary = req.decoded.summary;
        expect(req.context).toEqual({ title: 'Pay rent' });
        expect(req.envelope.nonce).toBe(3);
        expect(req.network.chainId).toBe(3);
      }
      void h.wallet.decideApproval(req.id, true);
    };
    await h.provider.connect();
    const to = Keypair.fromSeed(2).address;
    const r = await h.provider.signAction({ envelope: { signer: VECTOR.address0, nonce: 3, chain_id: 3, action: { Transfer: { to, asset: 'KEEL', amount: 2_500_000n } } }, context: { title: 'Pay rent' } });
    expect(summary).toBe(`Send 2.5 KEEL to ${to.slice(0, 8)}…${to.slice(-6)}`);
    expect(verify(r.signed as unknown as SdkSignedAction)).toBe(true);
    expect(r.tx_id).toBe(txId(r.signed as unknown as SdkSignedAction));
    expect(r.signature).toBe(r.signed.signature);
    const action = r.signed.envelope.action as { Transfer: { amount: unknown } };
    expect(action.Transfer.amount).toBe(2_500_000n);
  });

  it('INVALID_REQUEST for a foreign signer or a malformed envelope', async () => {
    await setup(h);
    autoDecide(h, true);
    await h.provider.connect();
    const foreign = Keypair.fromSeed(3).address;
    expect((await rejection(h.provider.signAction({ envelope: { signer: foreign, nonce: 0, chain_id: 3, action: 'ClaimRewards' } }))).code).toBe('INVALID_REQUEST');
    expect((await rejection(h.provider.signAction({ envelope: { signer: VECTOR.address0, nonce: -1, chain_id: 3, action: 'ClaimRewards' } }))).code).toBe('INVALID_REQUEST');
    expect((await rejection(h.provider.signAction({ envelope: { signer: VECTOR.address0, nonce: 0, chain_id: 3, action: { Transfer: { to: 'zz', asset: 'KEEL', amount: 1 } } } }))).code).toBe('INVALID_REQUEST');
    expect((await rejection(h.provider.signAction({ envelope: { signer: VECTOR.address0, nonce: 0, chain_id: 3, action: { Nope: {} } } }))).code).toBe('INVALID_REQUEST');
    expect((await rejection((h.provider as unknown as { signAction: (x: unknown) => Promise<unknown> }).signAction(null))).code).toBe('INVALID_REQUEST');
  });

  it('WRONG_NETWORK for a mismatched chain id or requested network', async () => {
    await setup(h);
    autoDecide(h, true);
    expect((await rejection(h.provider.connect({ network: 'devnet' }))).code).toBe('WRONG_NETWORK');
    expect((await rejection(h.provider.connect({ network: 'nope' }))).code).toBe('INVALID_REQUEST');
    await h.provider.connect({ network: 'testnet' });
    expect((await rejection(h.provider.signAction({ envelope: { signer: VECTOR.address0, nonce: 0, chain_id: 9, action: 'ClaimRewards' } }))).code).toBe('WRONG_NETWORK');
    expect((await rejection(h.provider.authorizeSession({ key: VECTOR.address1, scope: ['markets'], expires_at: now / 1000 + 3600, nonce: 0, chain_id: 9 }))).code).toBe('WRONG_NETWORK');
  });

  it('authorizeSession builds AuthorizeSessionKey and explains it in plain words', async () => {
    await setup(h);
    let sentence = '';
    h.approvals.onAdded = (req) => {
      if (req.kind === 'authorizeSession') {
        sentence = req.sentence;
        expect(req.scope).toEqual(['markets']);
        expect(req.key).toBe(VECTOR.address1);
        expect(req.decoded.kind).toBe('AuthorizeSessionKey');
      }
      void h.wallet.decideApproval(req.id, true);
    };
    await h.provider.connect();
    const expires_at = Math.floor(now / 1000) + 86400;
    const r = await h.provider.authorizeSession({ key: VECTOR.address1, scope: ['markets'], expires_at, nonce: 4, chain_id: 3 });
    expect(sentence).toMatch(new RegExp(`^Allow ${ORIGIN} to place and cancel orders for you until .+\\. It can never move funds\\.$`));
    expect(r.signed.envelope.action).toEqual({ AuthorizeSessionKey: { key: VECTOR.address1, scope: 1, expires_at } });
    expect(r.signed.envelope.nonce).toBe(4);
    expect(verify(r.signed as unknown as SdkSignedAction)).toBe(true);
    expect(r.tx_id).toBe(txId(r.signed as unknown as SdkSignedAction));
    // Validation.
    expect((await rejection(h.provider.authorizeSession({ key: VECTOR.address1, scope: [], expires_at, nonce: 5, chain_id: 3 }))).code).toBe('INVALID_REQUEST');
    expect((await rejection(h.provider.authorizeSession({ key: 'abc', scope: ['markets'], expires_at, nonce: 5, chain_id: 3 }))).code).toBe('INVALID_REQUEST');
    expect((await rejection(h.provider.authorizeSession({ key: VECTOR.address1, scope: ['markets'], expires_at: Math.floor(now / 1000) - 1, nonce: 5, chain_id: 3 }))).code).toBe('INVALID_REQUEST');
    expect((await rejection(h.provider.authorizeSession({ key: VECTOR.address1, scope: ['markets'], expires_at: Math.floor(now / 1000) + 31 * 86400, nonce: 5, chain_id: 3 }))).code).toBe('INVALID_REQUEST');
  });

  it('emits accountChanged / networkChanged to connected origins', async () => {
    await setup(h);
    autoDecide(h, true);
    await h.provider.connect();
    await h.wallet.addAccount();
    await h.wallet.selectAccount(1);
    expect(h.events.at(-1)).toEqual({ event: 'accountChanged', payload: { address: VECTOR.address1, network: 'testnet', chainId: 3 } });
    expect(await h.provider.getAccount()).toMatchObject({ address: VECTOR.address1 });
    await h.wallet.switchNetwork('devnet');
    expect(h.events.slice(-2)).toEqual([
      { event: 'networkChanged', payload: { network: 'devnet', chainId: 1 } },
      { event: 'accountChanged', payload: null },
    ]);
    expect(await h.provider.getAccount()).toBeNull();
    await h.wallet.switchNetwork('testnet');
    expect(await h.provider.getAccount()).toMatchObject({ address: VECTOR.address1, network: 'testnet' });
  });

  it('rejects requests from non-http origins', async () => {
    const bad = harness('chrome-extension://abc');
    await setup(bad);
    expect((await rejection(bad.provider.getAccount())).code).toBe('INVALID_REQUEST');
  });
});
