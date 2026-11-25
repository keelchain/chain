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
    const v = decodeAction({ PlaceOrder: { pair: 'BTC-USDT', side: 'buy', order_type: 'limit', price: 50_000_000_000, quantity: 200_000, quote_budget: null, client_id: 1 } });
    expect(v.label.toLowerCase()).toContain('order');
    const q = v.fields.find((f) => f.name === 'Quantity')!.field;
    expect(q).toMatchObject({ k: 'amount', asset: 'BTC' });
  });
  it('falls back to a generic view for unknown actions', () => {
    const v = decodeAction({ FutureThing: { x: 1 } });
    expect(v.kind).toBe('FutureThing');
    expect(v.fields.length).toBeGreaterThan(0);
    expect(decodeAction(undefined, 'Attest').kind).toBe('Attest');
  });
});

describe('client plan actions', () => {
  it('decodes BuyPlan with its tier and what it is paid in', () => {
    const v = decodeAction({ BuyPlan: { tier: 2 } });
    expect(v.label).toBe('Buy client plan');
    expect(v.summary).toBe('Buy client plan, tier 2');
    expect(v.fields).toEqual([
      { name: 'Tier', field: { k: 'number', value: 2 } },
      { name: 'Paid in', field: { k: 'asset', value: 'USDT' } },
    ]);
    expect(v.feeNote).toContain('plan fee');
  });
  it('decodes SetPlanRenewal, with tier 0 as stopping renewal', () => {
    expect(decodeAction({ SetPlanRenewal: { tier: 1 } }).summary).toBe('Renew the client plan at tier 1');
    const stop = decodeAction({ SetPlanRenewal: { tier: 0 } });
    expect(stop.label).toBe('Set plan renewal');
    expect(stop.summary).toBe('Stop renewing the client plan');
    expect(stop.fields).toEqual([{ name: 'Renews at', field: { k: 'text', value: 'Does not renew' } }]);
  });
  it('decodes the lock and the retail schedule a client signs', () => {
    expect(decodeAction({ LockBudget: { amount: '100000000000' } }).fields).toEqual([{ name: 'Amount', field: { k: 'amount', value: '100000000000', asset: 'KEEL' } }]);
    expect(decodeAction({ UnlockBudget: { amount: 5 } }).label).toBe('Unlock KEEL');
    expect(decodeAction({ SetClientFee: { p2p_bps: 50, taker_bps: 10, withdraw_bps: 0 } }).fields.map((f) => f.field)).toEqual([
      { k: 'bps', value: 50 },
      { k: 'bps', value: 10 },
      { k: 'bps', value: 0 },
    ]);
  });
  it('reads the plan and usage events in the billing asset', () => {
    const client = 'c1'.repeat(32);
    const started = decodeEvent({ type: 'ClientPlanStarted', client, tier: 2, asset: 'USDT', fee: '1000000000', allowance: '1200000000', ends_at: 1_800_000_000 });
    const by = Object.fromEntries(started.fields.map((f) => [f.name, f.field]));
    expect(by['Client']).toEqual({ k: 'address', value: client });
    expect(by['Fee']).toEqual({ k: 'amount', value: '1000000000', asset: 'USDT' });
    expect(by['Allowance']).toEqual({ k: 'amount', value: '1200000000', asset: 'USDT' });
    expect(by['Period ends']).toEqual({ k: 'timestamp', value: 1_800_000_000_000 });
    const covered = decodeEvent({ type: 'UsageCovered', client, amount: '20000', kind: 'fill' });
    expect(covered.fields.find((f) => f.name === 'Amount')!.field).toEqual({ k: 'amount', value: '20000', asset: 'USDT' });
  });
});

describe('decodeEvent', () => {
  it('labels flattened events and keeps their fields', () => {
    const e = decodeEvent({ type: 'OrderFilled', order_id: 11, maker_order_id: 10, pair: 'BTC-USDT', price: 50_000_000_000, quantity: 100_000, quote: 50_000_000, fee: 100 });
    expect(e.label.length).toBeGreaterThan(0);
    expect(JSON.stringify(e)).toContain('BTC-USDT');
  });
});

describe('client profile', () => {
  it('decodes SetClientProfile with the name and only the fields that are set', () => {
    const v = decodeAction({ SetClientProfile: { name: 'Acme Exchange', website: 'acme.example', logo: '' } });
    expect(v.label).toBe('Set client profile');
    expect(v.summary).toBe('Set the public profile to Acme Exchange');
    expect(v.fields.map((f) => f.name)).toEqual(['Name', 'Website']);
  });
});
