import { useHistory } from '../api/hooks';
import { MeterRow, type MeterTone } from '../components/Meter';
import { Amount, Badge, Card, ErrorState, Loading, TimeAgo } from '../components/ui';
import { big, money, ratio } from '../lib/amounts';
import { billingRows, type BillingRow } from '../lib/history';
import { useNetwork } from '../network/NetworkContext';
import { explorerUrl } from '../network/networks';
import { useHarbor, type Harbor } from './ClientGate';
import { OwedNotice, day } from './shared';

function Prices({ h }: { h: Harbor }) {
  const c = h.client;
  const asset = c.billing_asset;
  const p = c.usage_prices;
  const rows: [string, string, string][] = [
    ['Deposit address', 'Each address issued to one of your accounts', p.address_usd_micro],
    ['Payout', 'Each withdrawal queued, and each float swap', p.outbound_usd_micro],
    ['Order fill', 'Each order of one of your users that filled', p.fill_usd_micro],
    ['Escrow release', 'Each P2P trade released', p.release_usd_micro],
    ['Dispute ruling', 'Each dispute ruled', p.ruling_usd_micro],
  ];
  return (
    <Card title="Prices" flush>
      <div className="table-wrap">
        <table className="tbl wrap">
          <thead>
            <tr>
              <th>Usage</th>
              <th>Charged for</th>
              <th className="num">Price</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(([name, what, price]) => (
              <tr key={name}>
                <td>{name}</td>
                <td className="text-2">{what}</td>
                <td className="num">
                  <Amount value={price} asset={asset} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="card-body small text-2">
        Governance sets these prices in dollars; they are paid in {asset}, from the plan's included usage first.{' '}
        {c.protocol_fee_waived
          ? 'With a running plan and nothing owed, your users pay no protocol fee on these flows; you pay the price above instead.'
          : 'Protocol fees are not waived on this network: your users pay them, and you pay for addresses and payouts.'}{' '}
        An address or a payout you cannot pay for is refused; a fill, a release or a ruling never is, and what you cannot pay is owed.
      </div>
    </Card>
  );
}

function Period({ h }: { h: Harbor }) {
  const c = h.client;
  const plan = c.plan;
  const asset = c.billing_asset;
  const running = h.status === 'active' || h.status === 'suspended';
  if (!plan || !running) {
    return (
      <Card title="This period">
        <p className="text-2">No period is running, so no usage is included: every charge comes out of your {asset} balance.</p>
      </Card>
    );
  }
  const total = big(plan.allowance_usd_micro);
  const left = big(plan.allowance_left_usd_micro);
  const used = total - left;
  const tone: MeterTone = left === 0n ? 'bad' : ratio(left, total) < 0.2 ? 'warn' : 'ok';
  return (
    <Card title="This period">
      <div className="meter-list">
        <MeterRow
          label="Included usage used"
          ratio={ratio(used, total)}
          tone={tone}
          state={left === 0n ? 'Used up' : 'Running low'}
          figure={
            <>
              {money(used, asset)} of {money(total, asset)}
            </>
          }
          note={`${money(left, asset)} left until ${day(plan.ends_at)}. What is not used does not carry over.${h.status === 'suspended' ? ' The plan is suspended, so the allowance pays for nothing until the lock is back.' : ''}`}
        />
      </div>
    </Card>
  );
}

function Totals({ h }: { h: Harbor }) {
  const c = h.client;
  return (
    <Card title="Paid so far" flush>
      {c.usage_paid.length === 0 ? (
        <div className="empty">Nothing has been charged to this account yet.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>Asset</th>
                <th className="num">Plan fees and usage paid</th>
              </tr>
            </thead>
            <tbody>
              {c.usage_paid.map((u) => (
                <tr key={u.asset}>
                  <td>{u.asset}</td>
                  <td className="num">
                    <Amount value={u.amount} asset={u.asset} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="card-body small text-2">
        Owed now: <Amount value={c.usage_owed} asset={c.billing_asset} />
      </div>
    </Card>
  );
}

const SOURCE: Record<NonNullable<BillingRow['source']>, { label: string; tone: 'neutral' | 'accent' | 'bad' }> = {
  balance: { label: 'From balance', tone: 'neutral' },
  allowance: { label: 'From allowance', tone: 'accent' },
  owed: { label: 'Owed', tone: 'bad' },
};

function History({ h }: { h: Harbor }) {
  const { network } = useNetwork();
  const q = useHistory(h.address);
  const asset = h.client.billing_asset;
  const rows = billingRows(q.data?.pages.flatMap((p) => p.txs) ?? [], h.address);
  return (
    <Card title="Billing history" flush>
      {q.isError ? (
        <div className="card-body">
          <ErrorState error={q.error} what="billing history" />
        </div>
      ) : q.isPending ? (
        <Loading rows={4} />
      ) : rows.length === 0 ? (
        <div className="empty">No plan or usage events in the transactions read so far.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>When</th>
                <th>What</th>
                <th>Paid from</th>
                <th className="num">Amount</th>
                <th>Transaction</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.key}>
                  <td>
                    <TimeAgo ts={r.timestamp} />
                  </td>
                  <td>{r.what}</td>
                  <td>{r.source ? <Badge tone={SOURCE[r.source].tone}>{SOURCE[r.source].label}</Badge> : <span className="muted">—</span>}</td>
                  <td className="num">{r.amount === null ? <span className="muted">—</span> : <Amount value={r.amount} asset={asset} />}</td>
                  <td>
                    <a className="mono small" href={explorerUrl(network.id, `/tx/${r.tx_id}`)} target="_blank" rel="noreferrer noopener">
                      {r.tx_id.slice(0, 10)}… ↗
                    </a>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {q.hasNextPage && (
        <div className="card-body center">
          <button type="button" className="btn" onClick={() => void q.fetchNextPage()} disabled={q.isFetchingNextPage}>
            {q.isFetchingNextPage ? 'Loading…' : 'Load older'}
          </button>
        </div>
      )}
      <div className="card-body small text-2">
        Read from the indexer: the plan and usage events in transactions that name this account. Charges the chain makes at the end of a block, such as an automatic renewal or collecting what was
        owed, are not in a transaction and do not appear here; they show in the totals above.
      </div>
    </Card>
  );
}

export function UsagePage() {
  const h = useHarbor();
  return (
    <>
      <div className="page-head">
        <h1>Usage</h1>
        <span className="sub">What your users' activity costs, and what has been charged.</span>
      </div>
      <OwedNotice client={h.client} funds={h.funds} />
      <div className="grid-2">
        <Period h={h} />
        <Totals h={h} />
      </div>
      <Prices h={h} />
      <History h={h} />
    </>
  );
}
