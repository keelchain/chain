import { useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { useCandles, useFills, useMarket, useMarkets, useOrder } from '../api/hooks';
import type { CandleInterval } from '../api/types';
import { CandleChart } from '../components/charts/CandleChart';
import { DepthChart } from '../components/charts/DepthChart';
import { FillsTable } from '../components/tables';
import { AddressLink, Amount, Badge, BlockLink, Card, ErrorState, KV, Loading, PairLink, Price, StatTile, StatusBadge, TxLink } from '../components/ui';
import { useDecimals } from '../lib/assets';
import { formatAmount, formatBps, formatInt, formatPrice, splitPair } from '../lib/format';
import { useHref } from '../network/NetworkContext';

export function MarketsPage() {
  const q = useMarkets();
  if (q.isLoading) return <Loading rows={6} />;
  if (q.isError) return <ErrorState error={q.error} what="markets" />;
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Markets</h1>
        <span className="sub">On-chain order books matched deterministically in every block; KUSD quotes every pair</span>
      </div>
      <Card flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>Pair</th>
                <th className="num">Last price</th>
                <th className="num">Best bid</th>
                <th className="num">Best ask</th>
                <th className="num">24h volume</th>
                <th className="num">24h trades</th>
              </tr>
            </thead>
            <tbody>
              {(q.data ?? []).map((m) => (
                <tr key={m.pair}>
                  <td>
                    <PairLink pair={m.pair} />
                  </td>
                  <td className="num">
                    <Price value={m.last_price} pair={m.pair} />
                  </td>
                  <td className="num up">
                    <Price value={m.best_bid} pair={m.pair} />
                  </td>
                  <td className="num down">
                    <Price value={m.best_ask} pair={m.pair} />
                  </td>
                  <td className="num">
                    <Amount value={m.volume_24h_quote} asset={m.quote} compact />
                  </td>
                  <td className="num">{formatInt(m.trades_24h)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}

const INTERVALS: CandleInterval[] = ['1m', '5m', '1h', '1d'];

export function MarketPage() {
  const { pair: raw = '' } = useParams();
  const pair = decodeURIComponent(raw);
  const { base, quote } = splitPair(pair);
  const dec = useDecimals();
  const [interval, setInterval] = useState<CandleInterval>('1h');
  const q = useMarket(pair);
  const fills = useFills(pair, { limit: 30 });
  const candles = useCandles(pair, interval);
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="market" />;
  const m = q.data;
  const bd = m.base_decimals ?? dec(base);
  const qd = m.quote_decimals ?? dec(quote);
  const spread = m.best_bid && m.best_ask ? Number(BigInt(m.best_ask) - BigInt(m.best_bid)) / Number(BigInt(m.best_ask)) : null;
  const maxBid = Math.max(...m.book.bids.map(([, s]) => Number(BigInt(s))), 1);
  const maxAsk = Math.max(...m.book.asks.map(([, s]) => Number(BigInt(s))), 1);
  return (
    <div className="stack">
      <div className="page-head">
        <h1>{pair}</h1>
        <span className="sub">
          <Link to={`../assets/${encodeURIComponent(base)}`} relative="path">{base}</Link> priced in <Link to={`../assets/${encodeURIComponent(quote)}`} relative="path">{quote}</Link>
        </span>
      </div>
      <div className="tiles">
        <StatTile label="Last price" value={<Price value={m.last_price} pair={pair} />} />
        <StatTile label="Best bid" value={<Price value={m.best_bid} pair={pair} />} />
        <StatTile label="Best ask" value={<Price value={m.best_ask} pair={pair} />} hint={spread !== null ? `spread ${(spread * 100).toFixed(3)}%` : undefined} />
        <StatTile label="24h volume" value={<Amount value={m.volume_24h_quote} asset={quote} compact />} hint={<Amount value={m.volume_24h_base} asset={base} compact />} />
        <StatTile label="24h fills" value={formatInt(m.trades_24h)} />
        <StatTile label="Fees" value={`${formatBps(m.taker_fee_bps ?? 10)} taker`} hint={`${formatBps(m.maker_fee_bps ?? 0)} maker · no gas`} />
      </div>
      <Card
        title="Price"
        actions={
          <div className="row">
            {INTERVALS.map((i) => (
              <button key={i} type="button" className={`btn small${i === interval ? ' primary' : ''}`} onClick={() => setInterval(i)} aria-pressed={i === interval}>
                {i}
              </button>
            ))}
          </div>
        }
      >
        {candles.isLoading ? <Loading rows={4} /> : candles.isError ? <ErrorState error={candles.error} what="candles" /> : <CandleChart candles={candles.data ?? []} baseDecimals={bd} quoteDecimals={qd} base={base} quote={quote} />}
      </Card>
      <div className="grid-2">
        <Card title="Depth">
          <DepthChart bids={m.book.bids} asks={m.book.asks} baseDecimals={bd} quoteDecimals={qd} base={base} quote={quote} />
          {m.house_quote && (m.house_quote.bid || m.house_quote.ask) && (
            <div className="small text-2" style={{ marginTop: 10 }}>
              House quote: {m.house_quote.bid ? `bid ${formatPrice(m.house_quote.bid[0], qd)} × ${formatAmount(m.house_quote.bid[1], bd, { maxFraction: 4 })}` : ''}
              {m.house_quote.bid && m.house_quote.ask ? ' · ' : ''}
              {m.house_quote.ask ? `ask ${formatPrice(m.house_quote.ask[0], qd)} × ${formatAmount(m.house_quote.ask[1], bd, { maxFraction: 4 })}` : ''}
              {m.house_quote.valid_until ? <> · valid until block <BlockLink height={m.house_quote.valid_until} /></> : null}. The house level fills only when strictly better than the book.
            </div>
          )}
        </Card>
        <Card title="Order book" flush>
          <div className="book" style={{ padding: 8 }}>
            <div>
              <div className="book-row muted">
                <span>Size ({base})</span>
                <span>Total</span>
                <span>Bid</span>
              </div>
              {cumulate(m.book.bids).map(([p, s, c], i) => (
                <div className="book-row bid" key={`b${i}`}>
                  <span className="bar" style={{ width: `${(Number(BigInt(s)) / maxBid) * 100}%` }} />
                  <span>{formatAmount(s, bd, { maxFraction: 4 })}</span>
                  <span className="muted">{formatAmount(c, bd, { maxFraction: 3 })}</span>
                  <span className="up">{formatPrice(p, qd)}</span>
                </div>
              ))}
            </div>
            <div>
              <div className="book-row muted">
                <span>Ask</span>
                <span>Total</span>
                <span>Size ({base})</span>
              </div>
              {cumulate(m.book.asks).map(([p, s, c], i) => (
                <div className="book-row ask" key={`a${i}`}>
                  <span className="bar" style={{ width: `${(Number(BigInt(s)) / maxAsk) * 100}%` }} />
                  <span className="down">{formatPrice(p, qd)}</span>
                  <span className="muted">{formatAmount(c, bd, { maxFraction: 3 })}</span>
                  <span>{formatAmount(s, bd, { maxFraction: 4 })}</span>
                </div>
              ))}
            </div>
          </div>
        </Card>
      </div>
      <Card title="Last fills" flush>
        {fills.isLoading ? <Loading rows={6} /> : fills.isError ? <div className="card-body"><ErrorState error={fills.error} what="fills" /></div> : <FillsTable fills={fills.data?.fills ?? []} pair={pair} />}
      </Card>
    </div>
  );
}

function cumulate(levels: [string, string][]): [string, string, string][] {
  let c = 0n;
  return levels.slice(0, 15).map(([p, s]) => {
    c += BigInt(s);
    return [p, s, c.toString()];
  });
}

export function OrderPage() {
  const { id = '' } = useParams();
  const href = useHref();
  const q = useOrder(Number(id));
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="order" />;
  const o = q.data;
  const { base } = splitPair(o.pair);
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Order #{o.id}</h1>
        <StatusBadge status={o.status} />
        <span className="sub">
          <Badge tone={o.side === 'buy' ? 'good' : 'bad'}>{o.side}</Badge> {o.order_type} on <PairLink pair={o.pair} />
        </span>
      </div>
      <Card flush>
        <KV
          rows={[
            ['Owner', <AddressLink address={o.owner} full />],
            ['Pair', <PairLink pair={o.pair} />],
            ['Price', o.price ? <Price value={o.price} pair={o.pair} /> : 'market'],
            ['Quantity', <Amount value={o.quantity} asset={base} />],
            ['Filled', <Amount value={o.filled} asset={base} />],
            ['Created', <span className="row"><BlockLink height={o.created_height} /><TxLink id={o.tx_id} /></span>],
            ['Fees', <span className="text-2">No fee to place or cancel. The taker fee is taken from what the taker receives on each fill; see <Link to={href('/governance/params')}>parameters</Link>.</span>],
          ]}
        />
      </Card>
      <Card title={`Fills (${o.fills.length})`} flush>
        <FillsTable fills={o.fills} pair={o.pair} />
      </Card>
    </div>
  );
}
