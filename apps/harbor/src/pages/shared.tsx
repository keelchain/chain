/** Pieces more than one page shows: the plan's state as a badge, dates, the owed callout. */
import { Link } from 'react-router-dom';
import type { ClientRecord } from '../api/types';
import { Badge, Notice, type Tone } from '../components/ui';
import { big, money, shortfall } from '../lib/amounts';
import type { Funds, PlanStatus } from '../lib/plan';
import { useHref } from '../network/NetworkContext';

const STATUS: Record<PlanStatus, { label: string; tone: Tone }> = {
  active: { label: 'Active', tone: 'good' },
  suspended: { label: 'Suspended', tone: 'bad' },
  expired: { label: 'Expired', tone: 'warn' },
  none: { label: 'No plan', tone: 'neutral' },
};

export function StatusBadge({ status }: { status: PlanStatus }) {
  const s = STATUS[status];
  return <Badge tone={s.tone}>{s.label}</Badge>;
}

/** A block time (seconds) as a local date. */
export function day(secs: number): string {
  return new Date(secs * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

export function dateTime(ms: number): string {
  return new Date(ms).toLocaleString(undefined, { year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/** What is owed and what that closes, shown wherever money is decided. */
export function OwedNotice({ client, funds }: { client: ClientRecord; funds: Funds }) {
  const href = useHref();
  const owed = big(client.usage_owed);
  if (owed === 0n) return null;
  const asset = client.billing_asset;
  const missing = shortfall(owed, funds.usdt);
  return (
    <Notice
      tone="bad"
      title={`${money(owed, asset)} of usage is owed`}
      action={
        <Link className="btn" to={href('/funds')}>
          {missing > 0n ? `Add ${money(missing, asset)}` : 'Funds'}
        </Link>
      }
    >
      Until it is paid your users pay the protocol fees again, your pool pays for none of their actions, and no deposit address or payout is issued to them.{' '}
      {missing > 0n
        ? `The chain collects it from the next ${asset} that reaches this account.`
        : `The balance covers it: the chain collects it at the next epoch boundary or with the next charge, and buying a plan settles it first.`}
    </Notice>
  );
}
