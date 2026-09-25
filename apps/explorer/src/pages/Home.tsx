import { useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { useBlocks, useLiveBlocks, useStats, useTxs } from '../api/hooks';
import { SearchBox } from '../components/Layout';
import { BlocksTable, TxsTable } from '../components/tables';
import { Amount, Card, ErrorState, Loading, StatTile } from '../components/ui';
import { formatCompact, formatInt, formatMs, formatUsd } from '../lib/format';
import { useHref } from '../network/NetworkContext';

export function HomePage() {
  const href = useHref();
  const stats = useStats();
  const blocks = useBlocks({ limit: 10 }, true);
  const txs = useTxs({ limit: 10 }, true);
  const [tip, setTip] = useState<number | null>(null);
  const seen = useRef<number | null>(null);
  useLiveBlocks((m) => {
    if (m.type === 'block') {
      if (seen.current === null) seen.current = m.block.height;
      setTip(m.block.height);
    }
  });
  const highlightAbove = seen.current ?? undefined;
  const s = stats.data;
  const fee = s?.fees_24h.find((f) => f.asset === 'KUSD');

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Keelchain</h1>
        <span className="sub">Blocks, actions, accounts, markets, P2P offers, vaults and governance</span>
      </div>
      <div className="mobile-only">
        <SearchBox />
      </div>
      {stats.isError && <ErrorState error={stats.error} what="network statistics" />}
      <div className="tiles">
        <StatTile label="Height" value={s ? formatInt(tip ?? s.height) : <span className="skeleton" />} hint={s ? `${s.validators} validators` : undefined} />
        <StatTile label="Block time" value={s ? formatMs(s.block_time_ms_avg) : <span className="skeleton" />} hint="average, BFT finality" />
        <StatTile label="Actions / s" value={s ? s.tps_1h.toFixed(1) : <span className="skeleton" />} hint={s ? `${formatCompact(s.actions_24h)} in 24h` : undefined} />
        <StatTile label="Accounts" value={s ? formatCompact(s.accounts) : <span className="skeleton" />} hint="seen on chain" />
        <StatTile label="TVL" value={s ? formatUsd(s.tvl_usd, { compact: true }) : <span className="skeleton" />} hint="vault reserves at spot" />
        <StatTile label="Fees 24h" value={s ? (fee ? <Amount value={fee.amount} asset="KUSD" /> : '—') : <span className="skeleton" />} hint={s ? `+${s.fees_24h.filter((f) => f.asset !== 'KUSD').length} other assets` : undefined} />
      </div>
      <div className="grid-2">
        <Card
          title="Latest blocks"
          flush
          actions={
            <Link to={href('/blocks')} className="small">
              View all
            </Link>
          }
        >
          {blocks.isLoading ? <Loading rows={6} /> : blocks.isError ? <div className="card-body"><ErrorState error={blocks.error} what="blocks" /></div> : <BlocksTable blocks={blocks.data?.blocks ?? []} compact newest={highlightAbove} />}
        </Card>
        <Card
          title="Latest transactions"
          flush
          actions={
            <Link to={href('/txs')} className="small">
              View all
            </Link>
          }
        >
          {txs.isLoading ? <Loading rows={6} /> : txs.isError ? <div className="card-body"><ErrorState error={txs.error} what="transactions" /></div> : <TxsTable txs={txs.data?.txs ?? []} compact newest={highlightAbove} />}
        </Card>
      </div>
      {s && (
        <div className="grid-2">
          <Card title="Fees collected (24h)" flush>
            <div className="table-wrap">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Asset</th>
                    <th className="num">Amount</th>
                  </tr>
                </thead>
                <tbody>
                  {s.fees_24h.map((f) => (
                    <tr key={f.asset}>
                      <td>
                        <Link to={href(`/assets/${encodeURIComponent(f.asset)}`)}>{f.asset}</Link>
                      </td>
                      <td className="num">
                        <Amount value={f.amount} asset={f.asset} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="card-body small text-2">Fees are only taken on order-book fills (taker bps), P2P releases (seller fee) and withdrawals (network cost + flat fee). Placing orders, offers and trade steps cost nothing.</div>
          </Card>
          <Card title="Supply" flush>
            <div className="table-wrap">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Asset</th>
                    <th className="num">Supply</th>
                    <th className="num">Holders</th>
                  </tr>
                </thead>
                <tbody>
                  {s.assets.map((a) => (
                    <tr key={a.asset}>
                      <td>
                        <Link to={href(`/assets/${encodeURIComponent(a.asset)}`)}>{a.asset}</Link>
                      </td>
                      <td className="num">
                        <Amount value={a.supply} asset={a.asset} compact />
                      </td>
                      <td className="num">{formatInt(a.holders)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
