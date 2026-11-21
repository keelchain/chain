/**
 * Billing history out of the account's indexed transactions: the plan and
 * usage events that name this client, whoever signed the transaction (a
 * user's fill charges its client inside the user's transaction).
 */
import type { EventRecord, HistoryTx } from '../api/types';
import { big } from './amounts';

export type BillingType = 'ClientPlanStarted' | 'ClientPlanRenewalSet' | 'UsageCharged' | 'UsageCovered' | 'UsageOwed';
const TYPES: ReadonlySet<string> = new Set<BillingType>(['ClientPlanStarted', 'ClientPlanRenewalSet', 'UsageCharged', 'UsageCovered', 'UsageOwed']);

export interface BillingRow {
  key: string;
  tx_id: string;
  /** ms */
  timestamp: number;
  type: BillingType;
  /** What happened, in words. */
  what: string;
  /** USD micro; null when the event moves nothing (a renewal setting). */
  amount: bigint | null;
  /** Where the amount came from. */
  source: 'balance' | 'allowance' | 'owed' | null;
}

const KIND: Record<string, string> = {
  address: 'Deposit address',
  outbound: 'Payout',
  swap: 'Float swap',
  rotation: 'Vault rotation',
  fill: 'Order fill',
  release: 'Escrow release',
  ruling: 'Dispute ruling',
  owed: 'Settled what was owed',
};

function hexOf(v: unknown): string | null {
  if (typeof v === 'string') return v.toLowerCase();
  if (Array.isArray(v) && v.every((b) => typeof b === 'number')) return v.map((b) => (b as number).toString(16).padStart(2, '0')).join('');
  return null;
}

function amountOf(e: EventRecord, key: string): bigint {
  return big(e[key] as string | number | undefined);
}

function row(e: EventRecord): Pick<BillingRow, 'what' | 'amount' | 'source'> {
  const kind = KIND[String(e['kind'] ?? '')] ?? String(e['kind'] ?? 'Usage');
  switch (e.type as BillingType) {
    case 'ClientPlanStarted':
      return { what: `Tier ${String(e['tier'])} period started`, amount: amountOf(e, 'fee'), source: 'balance' };
    case 'ClientPlanRenewalSet':
      return { what: Number(e['tier']) === 0 ? 'Renewal stopped' : `Renewal set to tier ${String(e['tier'])}`, amount: null, source: null };
    case 'UsageCharged':
      return { what: kind, amount: amountOf(e, 'amount'), source: 'balance' };
    case 'UsageCovered':
      return { what: kind, amount: amountOf(e, 'amount'), source: 'allowance' };
    case 'UsageOwed':
      return { what: kind, amount: amountOf(e, 'amount'), source: 'owed' };
  }
}

/** The billing events of `client` in `txs`, in the order given (newest first). */
export function billingRows(txs: HistoryTx[], client: string): BillingRow[] {
  const me = client.toLowerCase();
  const out: BillingRow[] = [];
  for (const tx of txs) {
    tx.events.forEach((e, i) => {
      if (!TYPES.has(e.type) || hexOf(e['client']) !== me) return;
      out.push({ key: `${tx.tx_id}:${tx.height}:${i}`, tx_id: tx.tx_id, timestamp: tx.timestamp, type: e.type as BillingType, ...row(e) });
    });
  }
  return out;
}
