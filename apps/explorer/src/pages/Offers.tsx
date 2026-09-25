import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useAssets, useOffer, useOffers, useTrade } from '../api/hooks';
import type { Trade } from '../api/types';
import { OffersTable, TradesTable } from '../components/tables';
import { AddressLink, Amount, AssetLink, Badge, BlockLink, Card, ErrorState, Hex, KV, Loading, StatusBadge, TimeAgo, TxLink } from '../components/ui';
import { formatBps, formatDuration, formatExact } from '../lib/format';
import { useHref } from '../network/NetworkContext';

export function OffersPage() {
  const [sp, setSp] = useSearchParams();
  const asset = sp.get('asset') ?? '';
  const side = sp.get('side') ?? '';
  const status = sp.get('status') ?? '';
  const assets = useAssets();
  const q = useOffers({ asset: asset || undefined, side: (side || undefined) as 'buy' | 'sell' | undefined, status: status || undefined });
  const set = (k: string, v: string) => {
    const next = new URLSearchParams(sp);
    if (v) next.set(k, v);
    else next.delete(k);
    setSp(next);
  };
  return (
    <div className="stack">
      <div className="page-head">
        <h1>P2P offers</h1>
        <span className="sub">Public offer terms live on chain; payment instructions and chat stay end-to-end encrypted off chain</span>
      </div>
      <div className="filters">
        <select className="select" aria-label="Asset" value={asset} onChange={(e) => set('asset', e.target.value)}>
          <option value="">All assets</option>
          {(assets.data ?? []).map((a) => (
            <option key={a.asset} value={a.asset}>
              {a.asset}
            </option>
          ))}
        </select>
        <select className="select" aria-label="Side" value={side} onChange={(e) => set('side', e.target.value)}>
          <option value="">Buy and sell</option>
          <option value="buy">Buy</option>
          <option value="sell">Sell</option>
        </select>
        <select className="select" aria-label="Status" value={status} onChange={(e) => set('status', e.target.value)}>
          <option value="">Any status</option>
          <option value="active">Active</option>
          <option value="paused">Paused</option>
          <option value="closed">Closed</option>
        </select>
      </div>
      <Card flush>{q.isLoading ? <Loading rows={8} /> : q.isError ? <div className="card-body"><ErrorState error={q.error} what="offers" /></div> : <OffersTable offers={q.data?.offers ?? []} />}</Card>
    </div>
  );
}

export function OfferPage() {
  const { id = '' } = useParams();
  const q = useOffer(Number(id));
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="offer" />;
  const o = q.data;
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Offer #{o.id}</h1>
        <StatusBadge status={o.status} />
        <span className="sub">
          <Badge tone={o.side === 'sell' ? 'bad' : 'good'}>{o.side}</Badge> <AssetLink asset={o.asset} /> for {o.fiat_currency} via {o.payment_method}
        </span>
      </div>
      <Card flush>
        <KV
          rows={[
            ['Owner', <AddressLink address={o.owner} full />],
            ['Side', <Badge tone={o.side === 'sell' ? 'bad' : 'good'}>{o.side === 'sell' ? 'Selling' : 'Buying'} {o.asset}</Badge>],
            ['Fiat currency', o.fiat_currency],
            ['Payment method', o.payment_method],
            ['Margin over oracle spot', <span className={o.margin_bps < 0 ? 'down' : ''}>{o.margin_bps >= 0 ? '+' : ''}{formatBps(o.margin_bps)}</span>],
            ['Limits', <span><Amount value={o.min_amount} asset={o.asset} /> – <Amount value={o.max_amount} asset={o.asset} /></span>],
            ['Payment window', formatDuration(o.payment_window_secs)],
            ['Country', o.country ?? 'any'],
            ['Minimum tier', o.min_tier === 0 ? 'none' : `tier ${o.min_tier} (attested)`],
            ['Created', <BlockLink height={o.created_height} />],
            ['Deposit', <span className="text-2">A refundable KEEL offer deposit is locked while the offer is live and returned on close.</span>],
          ]}
        />
      </Card>
      <Card title={`Trades (${o.trades.length})`} flush>
        <TradesTable trades={o.trades} />
      </Card>
    </div>
  );
}

const STEPS: { id: string; label: string }[] = [
  { id: 'funded', label: 'Escrow funded' },
  { id: 'paid', label: 'Marked paid' },
  { id: 'released', label: 'Released' },
];

function stepsFor(t: Trade) {
  const reached = new Set((t.history ?? []).map((h) => h.status));
  reached.add(t.status);
  if (t.status === 'released') reached.add('paid');
  const terminal = t.status === 'cancelled' ? { id: 'cancelled', label: 'Cancelled' } : t.status === 'disputed' ? { id: 'disputed', label: 'In dispute' } : t.status === 'ruled' ? { id: 'ruled', label: 'Ruled by arbitrator' } : null;
  const list = terminal && t.status !== 'released' ? [...STEPS.filter((s) => s.id !== 'released'), terminal] : STEPS;
  return list.map((s) => ({ ...s, done: reached.has(s.id), current: s.id === t.status, bad: s.id === 'cancelled' }));
}

export function TradePage() {
  const { id = '' } = useParams();
  const href = useHref();
  const q = useTrade(Number(id));
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="trade" />;
  const t = q.data;
  const steps = stepsFor(t);
  const r = t.dispute?.ruling;
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Trade #{t.id}</h1>
        <StatusBadge status={t.status} />
        <span className="sub">
          <Amount value={t.amount} asset={t.asset} /> for <Amount value={t.fiat_amount} decimals={2} /> {t.fiat_currency} on <Link to={href(`/offers/${t.offer_id}`)}>offer #{t.offer_id}</Link>
        </span>
      </div>
      <Card>
        <div className="step-list">
          {steps.map((s) => (
            <span key={s.id} className={`step${s.done ? ' done' : ''}${s.current ? ' current' : ''}${s.bad && s.done ? ' bad' : ''}`}>
              <span className="dot" />
              {s.label}
            </span>
          ))}
        </div>
      </Card>
      <div className="grid-2">
        <Card title="Parties and terms" flush>
          <KV
            rows={[
              ['Buyer', <AddressLink address={t.buyer} full />],
              ['Seller', <AddressLink address={t.seller} full />],
              ['Escrow', <span><Amount value={t.amount} asset={t.asset} /> <span className="muted">(transferred into a restricted escrow account, not a lock)</span></span>],
              ['Fiat', <span><Amount value={t.fiat_amount} decimals={2} /> {t.fiat_currency}</span>],
              ['Seller fee', <span><Amount value={t.fee} asset={t.asset} /> <span className="muted">taken on release, split treasury / rewards / burn</span></span>],
              ['Started', <BlockLink height={t.started_height} />],
              ['Payment deadline', <span><TimeAgo ts={t.deadline} /> <span className="muted">({formatExact(t.deadline)})</span></span>],
              ['Paid at', t.paid_at ? formatExact(t.paid_at) : '—'],
              ['Closed', t.closed_height ? <BlockLink height={t.closed_height} /> : '—'],
            ]}
          />
        </Card>
        <Card title="Status history" flush>
          {t.history?.length ? (
            <ul className="timeline">
              {t.history.map((h, i) => (
                <li key={i} className={h.status === 'released' ? 'good' : h.status === 'cancelled' ? 'bad' : h.status === 'disputed' ? 'warn' : ''}>
                  <div className="ev-head">
                    <StatusBadge status={h.status} />
                    <BlockLink height={h.height} />
                    <TimeAgo ts={h.timestamp} />
                  </div>
                  <div className="ev-fields">
                    {h.by && <span><span className="name">by</span><AddressLink address={h.by} copy={false} /></span>}
                    {h.tx_id && <span><span className="name">tx</span><TxLink id={h.tx_id} /></span>}
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <div className="empty">No history recorded.</div>
          )}
        </Card>
      </div>
      {t.dispute && (
        <Card title="Dispute" flush>
          <KV
            rows={[
              ['Opened by', <AddressLink address={t.dispute.opened_by} full />],
              ['Opened at', <BlockLink height={t.dispute.opened_height} />],
              ['Evidence', <ul style={{ margin: 0, paddingLeft: 18 }}>{t.dispute.evidence.map((e, i) => <li key={i}><AddressLink address={e.by} copy={false} /> · <Hex value={e.hash} /> · <BlockLink height={e.height} /></li>)}</ul>],
              ['Ruling', r ? <span className="row"><Badge tone={r.kind === 'Split' ? 'warn' : 'accent'}>{r.kind === 'WinsBuyer' ? 'Wins buyer' : r.kind === 'WinsSeller' ? 'Wins seller' : `Split ${formatBps(r.buyer_bps ?? 0)} to buyer`}</Badge>{r.arbitrator && <AddressLink address={r.arbitrator} copy={false} />}<BlockLink height={r.height} /></span> : <Badge tone="warn">Awaiting arbitrator ruling</Badge>],
              ...(r ? [['Payout', <span><Amount value={r.buyer_amount} asset={t.asset} /> to buyer · <Amount value={r.seller_amount} asset={t.asset} /> to seller</span>] as [string, React.ReactNode]] : []),
              ['Rules', <span className="text-2">Bonded, governance-elected arbitrators rule on committed evidence hashes; the dispute fee is charged to the losing side; absent or provably wrong arbitrators are slashed.</span>],
            ]}
          />
        </Card>
      )}
    </div>
  );
}
