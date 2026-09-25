import { Link } from 'react-router-dom';
import type { Block, Fill, Offer, Trade, Transfer, Tx } from '../api/types';
import { actionLabel, MODULE_LABELS } from '../lib/decode';
import { formatBps, formatDuration, formatInt, splitPair } from '../lib/format';
import { useHref } from '../network/NetworkContext';
import { AddressLink, Amount, AssetLink, Badge, BlockLink, Empty, Hex, OkBadge, PairLink, Price, StatusBadge, TimeAgo, TxLink } from './ui';

export function BlocksTable({ blocks, compact = false, newest }: { blocks: Block[]; compact?: boolean; newest?: number }) {
  if (!blocks.length) return <Empty>No blocks.</Empty>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Height</th>
            <th>Age</th>
            <th className="num">Actions</th>
            {!compact && <th className="num">OK</th>}
            {!compact && <th className="num">Events</th>}
            <th>Proposer</th>
            {!compact && <th>State hash</th>}
          </tr>
        </thead>
        <tbody>
          {blocks.map((b) => (
            <tr key={b.height} className={newest !== undefined && b.height > newest ? 'new-row' : ''}>
              <td>
                <BlockLink height={b.height} />
              </td>
              <td>
                <TimeAgo ts={b.timestamp} />
              </td>
              <td className="num">{formatInt(b.tx_count)}</td>
              {!compact && <td className="num">{b.tx_count === b.ok_count ? <span className="muted">all</span> : <span className="down">{formatInt(b.ok_count)}</span>}</td>}
              {!compact && <td className="num">{formatInt(b.event_count)}</td>}
              <td>{b.proposer ? <AddressLink address={b.proposer} copy={false} /> : <span className="muted">—</span>}</td>
              {!compact && (
                <td>
                  <Hex value={b.state_hash} head={10} tail={6} />
                </td>
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function TxsTable({ txs, compact = false, hideSigner = false, newest }: { txs: Tx[]; compact?: boolean; hideSigner?: boolean; newest?: number }) {
  if (!txs.length) return <Empty>No transactions.</Empty>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Tx</th>
            <th>Action</th>
            {!compact && <th>Module</th>}
            {!hideSigner && <th>Signer</th>}
            <th>Block</th>
            <th>Age</th>
            <th>Result</th>
          </tr>
        </thead>
        <tbody>
          {txs.map((t) => (
            <tr key={t.tx_id} className={newest !== undefined && t.height > newest ? 'new-row' : ''}>
              <td>
                <TxLink id={t.tx_id} />
              </td>
              <td>{actionLabel(t.kind)}</td>
              {!compact && (
                <td>
                  <span className="text-2">{MODULE_LABELS[t.module] ?? t.module}</span>
                </td>
              )}
              {!hideSigner && (
                <td>
                  <AddressLink address={t.signer} copy={false} />
                </td>
              )}
              <td>
                <BlockLink height={t.height} />
              </td>
              <td>
                <TimeAgo ts={t.timestamp} />
              </td>
              <td>
                <OkBadge ok={t.ok} code={t.error?.code} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function TransfersTable({ transfers, self }: { transfers: Transfer[]; self?: string }) {
  if (!transfers.length) return <Empty>No transfers.</Empty>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Tx</th>
            <th>Kind</th>
            <th>From</th>
            <th>To</th>
            <th className="num">Amount</th>
            <th>Age</th>
          </tr>
        </thead>
        <tbody>
          {transfers.map((t, i) => {
            const out = self && t.from === self;
            return (
              <tr key={`${t.tx_id}-${i}`}>
                <td>
                  <TxLink id={t.tx_id} />
                </td>
                <td>
                  <Badge tone={out ? 'warn' : 'good'}>{self ? (out ? 'OUT' : 'IN') : t.kind}</Badge>
                </td>
                <td>
                  <AddressLink address={t.from} copy={false} />
                </td>
                <td>
                  <AddressLink address={t.to} copy={false} />
                </td>
                <td className="num">
                  <Amount value={t.amount} asset={t.asset} decimals={t.decimals} />
                </td>
                <td>
                  <TimeAgo ts={t.timestamp} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function FillsTable({ fills, pair }: { fills: Fill[]; pair: string }) {
  const { base, quote } = splitPair(pair);
  if (!fills.length) return <Empty>No fills yet.</Empty>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Age</th>
            <th>Side</th>
            <th className="num">Price</th>
            <th className="num">Quantity</th>
            <th className="num">Quote</th>
            <th className="num">Fee</th>
            <th>Taker</th>
            <th>Tx</th>
          </tr>
        </thead>
        <tbody>
          {fills.map((f, i) => (
            <tr key={`${f.tx_id}-${i}`}>
              <td>
                <TimeAgo ts={f.timestamp} />
              </td>
              <td>{f.taker_side ? <Badge tone={f.taker_side === 'buy' ? 'good' : 'bad'}>{f.taker_side}</Badge> : <span className="muted">—</span>}</td>
              <td className={`num ${f.taker_side === 'sell' ? 'down' : 'up'}`}>
                <Price value={f.price} pair={pair} />
              </td>
              <td className="num">
                <Amount value={f.quantity} asset={base} />
              </td>
              <td className="num">
                <Amount value={f.quote} asset={quote} />
              </td>
              <td className="num">
                <Amount value={f.fee} asset={f.taker_side === 'buy' ? base : quote} />
              </td>
              <td>
                <AddressLink address={f.taker} copy={false} />
              </td>
              <td>
                <TxLink id={f.tx_id} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function OffersTable({ offers, hideOwner = false }: { offers: Offer[]; hideOwner?: boolean }) {
  const href = useHref();
  if (!offers.length) return <Empty>No offers.</Empty>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Offer</th>
            <th>Side</th>
            <th>Asset</th>
            <th>Fiat</th>
            <th>Payment</th>
            <th className="num">Margin</th>
            <th className="num">Limits</th>
            <th>Window</th>
            {!hideOwner && <th>Owner</th>}
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          {offers.map((o) => (
            <tr key={o.id}>
              <td>
                <Link to={href(`/offers/${o.id}`)}>#{o.id}</Link>
              </td>
              <td>
                <Badge tone={o.side === 'sell' ? 'bad' : 'good'}>{o.side}</Badge>
              </td>
              <td>
                <AssetLink asset={o.asset} />
              </td>
              <td>{o.fiat_currency}</td>
              <td>{o.payment_method}</td>
              <td className={`num ${o.margin_bps >= 0 ? '' : 'down'}`}>
                {o.margin_bps >= 0 ? '+' : ''}
                {formatBps(o.margin_bps)}
              </td>
              <td className="num">
                <Amount value={o.min_amount} asset={o.asset} sym={false} /> – <Amount value={o.max_amount} asset={o.asset} />
              </td>
              <td>{formatDuration(o.payment_window_secs)}</td>
              {!hideOwner && (
                <td>
                  <AddressLink address={o.owner} copy={false} />
                </td>
              )}
              <td>
                <StatusBadge status={o.status} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function TradesTable({ trades, self }: { trades: Trade[]; self?: string }) {
  const href = useHref();
  if (!trades.length) return <Empty>No trades.</Empty>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Trade</th>
            <th>Offer</th>
            <th className="num">Amount</th>
            <th className="num">Fiat</th>
            <th>Buyer</th>
            <th>Seller</th>
            <th>Started</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          {trades.map((t) => (
            <tr key={t.id}>
              <td>
                <Link to={href(`/trades/${t.id}`)}>#{t.id}</Link>
              </td>
              <td>
                <Link to={href(`/offers/${t.offer_id}`)}>#{t.offer_id}</Link>
              </td>
              <td className="num">
                <Amount value={t.amount} asset={t.asset} />
              </td>
              <td className="num">
                <Amount value={t.fiat_amount} decimals={2} /> {t.fiat_currency}
              </td>
              <td>{self === t.buyer ? <Badge tone="accent">this account</Badge> : <AddressLink address={t.buyer} copy={false} />}</td>
              <td>{self === t.seller ? <Badge tone="accent">this account</Badge> : <AddressLink address={t.seller} copy={false} />}</td>
              <td>
                <BlockLink height={t.started_height} />
              </td>
              <td>
                <StatusBadge status={t.status} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function PairCell({ pair }: { pair: string }) {
  return <PairLink pair={pair} />;
}
