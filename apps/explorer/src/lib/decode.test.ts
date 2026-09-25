import { describe, expect, it } from 'vitest';
import { decodeAction, decodeEvent, untag } from './decode';

describe('decodeAction', () => {
  it('reads serde externally-tagged actions', () => {
    expect(untag({ Transfer: { to: 'aa', asset: 'KEEL', amount: 5 } })[0]).toBe('Transfer');
    expect(untag('ClaimRewards')[0]).toBe('ClaimRewards');
    const v = decodeAction({ Transfer: { to: 'ab'.repeat(32), asset: 'KEEL', amount: '1000000' } });
    expect(v.kind).toBe('Transfer');
    expect(v.fields.map((f) => f.name)).toEqual(['To', 'Amount']);
    expect(v.fields[1]!.field).toMatchObject({ k: 'amount', value: '1000000', asset: 'KEEL' });
  });
  it('decodes orders with a price and quantity in the pair assets', () => {
    const v = decodeAction({ PlaceOrder: { pair: 'BTC-KUSD', side: 'buy', order_type: 'limit', price: 50_000_000_000, quantity: 200_000, quote_budget: null, client_id: 1 } });
    expect(v.label.toLowerCase()).toContain('order');
    const q = v.fields.find((f) => f.name === 'Quantity')!.field;
    expect(q).toMatchObject({ k: 'amount', asset: 'BTC' });
  });
  it('falls back to a generic view for unknown actions', () => {
    const v = decodeAction({ FutureThing: { x: 1 } });
    expect(v.kind).toBe('FutureThing');
    expect(v.fields.length).toBeGreaterThan(0);
    expect(decodeAction(undefined, 'SetParam').kind).toBe('SetParam');
  });
});

describe('decodeEvent', () => {
  it('labels flattened events and keeps their fields', () => {
    const e = decodeEvent({ type: 'OrderFilled', order_id: 11, maker_order_id: 10, pair: 'BTC-KUSD', price: 50_000_000_000, quantity: 100_000, quote: 50_000_000, fee: 100 });
    expect(e.label.length).toBeGreaterThan(0);
    expect(JSON.stringify(e)).toContain('BTC-KUSD');
  });
});
