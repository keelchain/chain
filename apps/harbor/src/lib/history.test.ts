import { describe, expect, it } from 'vitest';
import type { HistoryTx } from '../api/types';
import { billingRows } from './history';

const me = 'c1'.repeat(32);
const other = 'd2'.repeat(32);
const tx = (id: string, events: HistoryTx['events']): HistoryTx => ({ tx_id: id.repeat(32), height: 10, timestamp: 1_000, signer: other, kind: 'PlaceOrder', ok: true, events });

describe('billingRows', () => {
  it('keeps the plan and usage events that name this client, with where the money came from', () => {
    const rows = billingRows(
      [
        tx('aa', [
          { type: 'OrderFilled', order_id: 1 },
          { type: 'UsageCovered', client: me, amount: '20000', kind: 'fill' },
          { type: 'UsageCharged', client: other, asset: 'USDT', amount: '20000', kind: 'fill' },
        ]),
        tx('bb', [{ type: 'UsageOwed', client: me, amount: 100000, kind: 'release' }]),
        tx('cc', [
          { type: 'UsageCharged', client: me, asset: 'USDT', amount: '37500000', kind: 'owed' },
          { type: 'ClientPlanStarted', client: me.toUpperCase(), tier: 2, asset: 'USDT', fee: '1000000000', allowance: '1200000000', ends_at: 5 },
        ]),
        tx('dd', [{ type: 'ClientPlanRenewalSet', client: me, tier: 0 }]),
      ],
      me,
    );
    expect(rows.map((r) => [r.what, r.source, r.amount])).toEqual([
      ['Order fill', 'allowance', 20_000n],
      ['Escrow release', 'owed', 100_000n],
      ['Settled what was owed', 'balance', 37_500_000n],
      ['Tier 2 period started', 'balance', 1_000_000_000n],
      ['Renewal stopped', null, null],
    ]);
  });
  it('reads a client given as bytes and ignores everything else', () => {
    const bytes = Array.from({ length: 32 }, () => 0xc1);
    expect(billingRows([tx('ee', [{ type: 'UsageCovered', client: bytes, amount: '1', kind: 'address' }])], me)).toHaveLength(1);
    expect(billingRows([tx('ff', [{ type: 'Transferred', from: me, to: other, amount: '1' }])], me)).toEqual([]);
  });
});
