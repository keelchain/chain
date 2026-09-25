import { useState } from 'react';
import { useParams } from 'react-router-dom';
import { useBlock, useBlocks, useLiveBlocks } from '../api/hooks';
import type { Block } from '../api/types';
import { EventsTimeline } from '../components/Events';
import { BlocksTable, TxsTable } from '../components/tables';
import { AddressLink, BlockLink, Card, ErrorState, Hex, KV, Loading, Pager, Tabs, TimeAgo } from '../components/ui';
import { formatExact, formatInt } from '../lib/format';

export function BlocksPage() {
  const [cursor, setCursor] = useState<string | undefined>(undefined);
  const [pages, setPages] = useState<Block[][]>([]);
  const q = useBlocks({ limit: 25, cursor }, cursor === undefined);
  useLiveBlocks();
  const current = q.data?.blocks ?? [];
  const all = cursor === undefined ? current : [...pages.flat(), ...current];
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Blocks</h1>
        <span className="sub">Newest first; every block carries ordered actions, observer attestations and proofs</span>
      </div>
      <Card flush>
        {q.isLoading && !all.length ? <Loading rows={8} /> : q.isError ? <div className="card-body"><ErrorState error={q.error} what="blocks" /></div> : <BlocksTable blocks={all} />}
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

export function BlockPage() {
  const { height: raw } = useParams();
  const height = Number(raw);
  const q = useBlock(height);
  const [tab, setTab] = useState<'txs' | 'events'>('txs');
  if (!Number.isInteger(height) || height < 1) return <ErrorState error={{ status: 404 }} what="block" />;
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="block" />;
  const b = q.data;
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Block {formatInt(b.height)}</h1>
        <span className="sub">
          <TimeAgo ts={b.timestamp} />
        </span>
        <span className="spacer" />
        <span className="row">
          {b.height > 1 && <BlockLink height={b.height - 1} />}
          <span className="muted">←</span>
          <span className="muted">→</span>
          <BlockLink height={b.height + 1} />
        </span>
      </div>
      <Card flush>
        <KV
          rows={[
            ['Height', formatInt(b.height)],
            ['Timestamp', `${formatExact(b.timestamp)} (BFT time, median of validator clocks)`],
            ['Actions', `${formatInt(b.tx_count)} (${formatInt(b.ok_count)} ok, ${formatInt(b.tx_count - b.ok_count)} failed)`],
            ['Events', formatInt(b.event_count)],
            ['Proposer', b.proposer ? <AddressLink address={b.proposer} full /> : '—'],
            ['State hash', <Hex value={b.state_hash} full />],
          ]}
        />
      </Card>
      <Card flush>
        <Tabs
          tabs={[
            { id: 'txs', label: `Actions (${b.receipts.length})` },
            { id: 'events', label: `Events (${b.events.length})` },
          ]}
          value={tab}
          onChange={setTab}
        />
        {tab === 'txs' ? <TxsTable txs={b.receipts} /> : <EventsTimeline events={b.events} />}
      </Card>
    </div>
  );
}
