import { describe, expect, it } from 'vitest';
import { decodeAction, sessionSentence } from './decode';

const ADDR = '1de352e44cd333672593f2334a730e180aaf290de89aa16d480de594e34e2961';

function field(d: ReturnType<typeof decodeAction>, name: string) {
  const f = d.fields.find((x) => x.name === name);
  if (!f) throw new Error(`missing field ${name} in ${d.fields.map((x) => x.name).join(',')}`);
  return f.field;
}

describe('decodeAction', () => {
  it('Transfer with KEEL decimals', () => {
    const d = decodeAction({ Transfer: { to: ADDR, asset: 'KEEL', amount: '1500000', memo: 'rent' } });
    expect(d.kind).toBe('Transfer');
    expect(d.summary).toBe('Send 1.5 KEEL to 1de352e4…4e2961');
    expect(field(d, 'Amount')).toEqual({ k: 'amount', value: '1500000', asset: 'KEEL', formatted: '1.5 KEEL' });
    expect(field(d, 'To')).toEqual({ k: 'address', value: ADDR });
    expect(field(d, 'Memo')).toEqual({ k: 'text', value: 'rent' });
    expect(d.warning).toMatch(/sends funds/);
  });

  it('Transfer with 18-decimal ETH and bigint amount', () => {
    const d = decodeAction({ Transfer: { to: ADDR, asset: 'ETH.ETH', amount: 1_250_000_000_000_000_000n } });
    expect(field(d, 'Amount')).toMatchObject({ formatted: '1.25 ETH.ETH' });
  });

  it('PlaceOrder uses base decimals for quantity and quote decimals for price', () => {
    const d = decodeAction({ PlaceOrder: { pair: 'BTC-KUSD', side: 'buy', order_type: 'limit', price: 65000_000000, quantity: 5_000_000, quote_budget: null, client_id: 7 } });
    expect(d.summary).toBe('Buy 0.05 BTC at 65,000 KUSD on BTC-KUSD');
    expect(field(d, 'Price')).toMatchObject({ k: 'price', formatted: '65,000 KUSD', pair: 'BTC-KUSD' });
    expect(field(d, 'Quantity')).toMatchObject({ formatted: '0.05 BTC', asset: 'BTC', value: '5000000' });
    expect(field(d, 'Side')).toMatchObject({ k: 'badge', value: 'Buy', tone: 'good' });
    expect(field(d, 'Client id')).toEqual({ k: 'number', value: '7' });
    const m = decodeAction({ PlaceOrder: { pair: 'BTC-KUSD', side: 'sell', order_type: 'market', quantity: '100000000' } });
    expect(m.summary).toBe('Sell 1 BTC at market on BTC-KUSD');
  });

  it('ReleaseTrade', () => {
    const d = decodeAction({ ReleaseTrade: { trade_id: 123 } });
    expect(d.label).toBe('Release escrow');
    expect(d.summary).toBe('Release escrow of trade #123');
    expect(field(d, 'Trade')).toEqual({ k: 'id', what: 'trade', id: '123' });
    expect(d.warning).toMatch(/releases escrowed funds/);
  });

  it('Withdraw with 8-decimal BTC', () => {
    const d = decodeAction({ Withdraw: { asset: 'BTC.BTC', to: 'bc1qxyz', amount: '5000000' } });
    expect(d.summary).toBe('Withdraw 0.05 BTC.BTC to bc1qxyz');
    expect(field(d, 'Destination')).toEqual({ k: 'external', chain: 'BTC', value: 'bc1qxyz' });
    expect(field(d, 'Amount')).toMatchObject({ formatted: '0.05 BTC.BTC' });
  });

  it('LockBudget / UnlockBudget in KEEL', () => {
    const d = decodeAction({ LockBudget: { amount: 25_000_000 } });
    expect(d.summary).toBe('Lock 25 KEEL as action budget');
    expect(field(d, 'Amount')).toMatchObject({ formatted: '25 KEEL' });
    expect(d.warning).toMatch(/locks KEEL/);
    expect(decodeAction({ UnlockBudget: { amount: '1000000' } }).summary).toBe('Unlock 1 KEEL of action budget');
  });

  it('AuthorizeSessionKey explains scope and expiry in plain words', () => {
    const exp = 1_800_000_000;
    const d = decodeAction({ AuthorizeSessionKey: { key: ADDR, scope: 3, expires_at: exp } });
    expect(d.label).toBe('Authorize session key');
    expect(d.summary).toBe(sessionSentence(['markets', 'p2p_manage'], exp, 'the session key'));
    expect(d.summary).toMatch(/^Allow the session key to place and cancel orders, and create, update, pause and close P2P offers and mark trades as paid for you until .+\. It can never move funds\.$/);
    expect(field(d, 'Session key')).toEqual({ k: 'address', value: ADDR });
    expect(field(d, 'Scope bits')).toEqual({ k: 'number', value: '3' });
    expect(field(d, 'Can move funds')).toMatchObject({ value: 'Never' });
    expect(decodeAction({ AuthorizeSessionKey: { key: ADDR, scope: 1, expires_at: exp } }).fields.find((f) => f.name === 'Allowed to')?.field).toEqual({ k: 'text', value: 'place and cancel orders' });
    expect(d.warning).toBeUndefined();
  });

  it('falls back to a generic listing for unknown kinds and unit variants', () => {
    expect(decodeAction('ClaimRewards').summary).toBe('Claim staking rewards');
    const d = decodeAction({ SomethingNew: { foo: 1, bar: true } });
    expect(d.kind).toBe('SomethingNew');
    expect(d.fields).toEqual([{ name: 'Foo', field: { k: 'text', value: '1' } }, { name: 'Bar', field: { k: 'bool', value: true } }]);
  });
});
