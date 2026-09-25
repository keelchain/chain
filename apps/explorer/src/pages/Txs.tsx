import { useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useLiveBlocks, useTx, useTxs } from '../api/hooks';
import type { Tx } from '../api/types';
import { EventsTimeline } from '../components/Events';
import { FieldValue } from '../components/FieldValue';
import { TxsTable } from '../components/tables';
import { AddressLink, Badge, BlockLink, Card, ErrorState, Hex, JsonView, KV, Loading, OkBadge, Pager, Tabs, TimeAgo } from '../components/ui';
import { decodeAction, MODULE_LABELS } from '../lib/decode';
import { formatExact, formatInt } from '../lib/format';
import { useHref } from '../network/NetworkContext';

const MODULES = Object.keys(MODULE_LABELS);

export function TxsPage() {
  const [sp, setSp] = useSearchParams();
  const module = sp.get('module') ?? '';
  const ok = sp.get('ok') ?? '';
  const [cursor, setCursor] = useState<string | undefined>(undefined);
  const [pages, setPages] = useState<Tx[][]>([]);
  const filter = { limit: 25, cursor, module: module || undefined, ok: ok === '' ? undefined : ok === '1' };
  const q = useTxs(filter, cursor === undefined);
  useLiveBlocks();
  const current = q.data?.txs ?? [];
  const all = cursor === undefined ? current : [...pages.flat(), ...current];
  const setFilter = (k: string, v: string) => {
    const next = new URLSearchParams(sp);
    if (v) next.set(k, v);
    else next.delete(k);
    setSp(next);
    setCursor(undefined);
    setPages([]);
  };
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Transactions</h1>
        <span className="sub">Signed actions in finalized blocks. A failed action still consumes its nonce and one unit of budget.</span>
      </div>
      <div className="filters">
        <select className="select" aria-label="Module" value={module} onChange={(e) => setFilter('module', e.target.value)}>
          <option value="">All modules</option>
          {MODULES.map((m) => (
            <option key={m} value={m}>
              {MODULE_LABELS[m]}
            </option>
          ))}
        </select>
        <select className="select" aria-label="Result" value={ok} onChange={(e) => setFilter('ok', e.target.value)}>
          <option value="">Any result</option>
          <option value="1">OK only</option>
          <option value="0">Failed only</option>
        </select>
      </div>
      <Card flush>
        {q.isLoading && !all.length ? <Loading rows={8} /> : q.isError ? <div className="card-body"><ErrorState error={q.error} what="transactions" /></div> : <TxsTable txs={all} />}
        <Pager
          hasMore={!!q.data?.next_cursor}
          loading={q.isFetching}
          onMore={() => {
            setPages((p) => [...p, current]);
            setCursor(q.data?.next_cursor ?? undefined);
          }}
        />
      </Card>
    </div>
  );
}

export function TxPage() {
  const { id = '' } = useParams();
  const href = useHref();
  const q = useTx(id.toLowerCase().replace(/^0x/, ''));
  const [tab, setTab] = useState<'decoded' | 'raw'>('decoded');
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="transaction" />;
  const t = q.data;
  const view = decodeAction(t.action, t.kind);
  return (
    <div className="stack">
      <div className="page-head">
        <h1>{view.label}</h1>
        <OkBadge ok={t.ok} code={t.error?.code} />
        <span className="sub">{view.summary}</span>
      </div>
      {!t.ok && t.error && (
        <div className="error-box" role="status">
          <strong>Action failed:</strong> <code>{t.error.code}</code> {t.error.message}. The nonce and one unit of action budget were still consumed; no state other than that changed.
        </div>
      )}
      <Card title="Overview" flush>
        <KV
          rows={[
            ['Transaction id', <Hex value={t.tx_id} full />],
            ['Block', <span className="row"><BlockLink height={t.height} /><span className="muted">index {t.index}</span></span>],
            ['Timestamp', <span><TimeAgo ts={t.timestamp} /> <span className="muted">({formatExact(t.timestamp)})</span></span>],
            ['Signer', <AddressLink address={t.signer} full />],
            ['Module', <Link to={`${href('/txs')}?module=${t.module}`}>{MODULE_LABELS[t.module] ?? t.module}</Link>],
            ['Action', <Badge tone="accent">{t.kind}</Badge>],
            ['Result', <OkBadge ok={t.ok} code={t.error?.code} />],
            ['Fee', <span className="text-2">{view.feeNote}</span>],
            ['Block state hash', <Hex value={t.block.state_hash} full />],
          ]}
        />
      </Card>
      <Card
        title="Action"
        flush
        actions={
          <Tabs
            tabs={[
              { id: 'decoded', label: 'Decoded' },
              { id: 'raw', label: 'Raw JSON' },
            ]}
            value={tab}
            onChange={setTab}
          />
        }
      >
        {tab === 'raw' ? (
          <div className="card-body">
            <JsonView value={t.action} />
          </div>
        ) : view.fields.length ? (
          <KV rows={view.fields.map((f) => [f.name, <FieldValue field={f.field} />])} />
        ) : (
          <div className="empty">This action carries no parameters.</div>
        )}
      </Card>
      <Card title={`Events (${formatInt(t.events.length)})`} flush>
        <EventsTimeline events={t.events} />
      </Card>
    </div>
  );
}
