import { useQueryClient } from '@tanstack/react-query';
import { useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import type { Receipt } from '@keelchain/sdk';
import { useAccount, useAccountOrders } from '../api/hooks';
import { formatAmount, formatPrice } from '../lib/format';
import { useHref } from '../network/NetworkContext';
import { toUnits } from '../pages/Dashboard';
import { useWallet } from '../wallet/WalletContext';
import { spendable } from './Connect';
import { Badge, Card } from './ui';

/** Place and cancel orders on one pair's book with Keel Wallet. The chain's
 *  own order book is the market: a limit order rests, a market order fills
 *  against what is there (the house quote included). No fee beyond the
 *  pair's taker fee, no gas: the action spends budget. */
export function TradeCard({ pair, base, quote, baseDecimals, quoteDecimals, lastPrice }: { pair: string; base: string; quote: string; baseDecimals: number; quoteDecimals: number; lastPrice: string | null }) {
  const w = useWallet();
  const qc = useQueryClient();
  const href = useHref();
  const addr = w.account?.address ?? '';
  const acct = useAccount(addr);
  const orders = useAccountOrders(addr);
  const [side, setSide] = useState<'buy' | 'sell'>('buy');
  const [kind, setKind] = useState<'limit' | 'market'>('limit');
  const [price, setPrice] = useState('');
  const [qty, setQty] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<Receipt | null>(null);

  const run = async (action: unknown, title: string) => {
    setBusy(true);
    setError(null);
    try {
      const r = await w.signAndSubmit(action, { title });
      setReceipt(r);
      await qc.invalidateQueries();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!w.account) {
    return (
      <Card title={<h2>Trade {pair}</h2>}>
        <p className="text-2 small">
          Orders go straight onto the chain's order book, signed in Keel Wallet. Connect to buy or sell {base} for {quote} here; the Keel Wallet holds the keys, the chain matches the orders, there is no middleman and no gas.
        </p>
        <div className="row">
          <button type="button" className="btn primary" onClick={() => void w.connect()} disabled={w.busy}>
            {w.busy ? 'Connecting…' : 'Connect Keel Wallet'}
          </button>
          {w.detected && !w.provider && (
            <a className="btn" href="https://keelchain.com/wallet/">
              Get the wallet
            </a>
          )}
          {w.error && <span className="small down">{w.error}</span>}
        </div>
      </Card>
    );
  }

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const quantity = toUnits(qty, baseDecimals);
    if (quantity === null || quantity === 0n) {
      setError(`Quantity must be a positive number with at most ${baseDecimals} decimals`);
      return;
    }
    let limit: bigint | null = null;
    if (kind === 'limit') {
      limit = toUnits(price, quoteDecimals);
      if (limit === null || limit === 0n) {
        setError(`Price must be a positive number with at most ${quoteDecimals} decimals`);
        return;
      }
    }
    void run(
      { PlaceOrder: { pair, side, order_type: kind, price: limit, quantity, quote_budget: null, client_id: null } },
      `${side === 'buy' ? 'Buy' : 'Sell'} ${qty} ${base} ${kind === 'limit' ? `at ${price} ${quote}` : 'at market'}`,
    );
  };

  const open = (orders.data ?? []).filter((o) => o.pair === pair && /open|partial/i.test(o.status));
  const balances = acct.data?.balances;
  return (
    <Card title={<h2>Trade {pair}</h2>}>
      <form className="form" onSubmit={submit}>
        <div className="row">
          <button type="button" className={`btn${side === 'buy' ? ' primary' : ''}`} onClick={() => setSide('buy')} aria-pressed={side === 'buy'}>
            Buy {base}
          </button>
          <button type="button" className={`btn${side === 'sell' ? ' primary' : ''}`} onClick={() => setSide('sell')} aria-pressed={side === 'sell'}>
            Sell {base}
          </button>
          <span className="small text-2" style={{ marginLeft: 'auto' }}>
            Available: {spendable(balances, base, baseDecimals)} {base} · {spendable(balances, quote, quoteDecimals)} {quote}
          </span>
        </div>
        <div className="row">
          <button type="button" className={`btn small${kind === 'limit' ? ' primary' : ''}`} onClick={() => setKind('limit')} aria-pressed={kind === 'limit'}>
            Limit
          </button>
          <button type="button" className={`btn small${kind === 'market' ? ' primary' : ''}`} onClick={() => setKind('market')} aria-pressed={kind === 'market'}>
            Market
          </button>
        </div>
        {kind === 'limit' && (
          <label className="field">
            <span className="field-label">Price ({quote} per {base})</span>
            <input className="input" value={price} onChange={(e) => setPrice(e.target.value)} inputMode="decimal" placeholder={lastPrice ? formatPrice(lastPrice, quoteDecimals) : ''} required />
            <span className="small text-2">A limit order rests on the book until it fills or you cancel it.</span>
          </label>
        )}
        <label className="field">
          <span className="field-label">Quantity ({base})</span>
          <input className="input" value={qty} onChange={(e) => setQty(e.target.value)} inputMode="decimal" required />
          {kind === 'market' && <span className="small text-2">Fills now against the best resting orders and the house quote; what cannot fill is dropped.</span>}
        </label>
        <div className="row">
          <button type="submit" className="btn primary" disabled={busy}>
            {busy ? 'Waiting for the wallet…' : side === 'buy' ? `Buy ${base}` : `Sell ${base}`}
          </button>
          {error && <span className="small down">{error}</span>}
        </div>
        {receipt && (
          <div className={`receipt ${receipt.ok ? 'ok' : 'bad'}`} role="status">
            {receipt.ok ? <Badge tone="good">Applied</Badge> : <Badge tone="bad">Refused</Badge>}{' '}
            <Link to={href(`/tx/${receipt.tx_id}`)} className="mono small">
              {receipt.tx_id.slice(0, 16)}…
            </Link>
            {!receipt.ok && receipt.error && <span className="small text-2"> {receipt.error.message}</span>}
          </div>
        )}
      </form>
      <h3 style={{ marginTop: 16 }}>Your open orders on {pair}</h3>
      {open.length === 0 ? (
        <div className="empty">None.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>Order</th>
                <th>Side</th>
                <th className="num">Price</th>
                <th className="num">Quantity</th>
                <th className="num">Filled</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {open.map((o) => (
                <tr key={o.id}>
                  <td>
                    <Link to={href(`/orders/${o.id}`)}>#{o.id}</Link>
                  </td>
                  <td>
                    <Badge tone={o.side === 'buy' ? 'good' : 'bad'}>{o.side}</Badge>
                  </td>
                  <td className="num">{o.price ? formatPrice(o.price, quoteDecimals) : 'market'}</td>
                  <td className="num">{formatAmount(o.quantity, baseDecimals, { maxFraction: 6 })}</td>
                  <td className="num">{formatAmount(o.filled, baseDecimals, { maxFraction: 6 })}</td>
                  <td className="num">
                    <button type="button" className="btn small" disabled={busy} onClick={() => void run({ CancelOrder: { order_id: o.id } }, `Cancel order #${o.id}`)}>
                      Cancel
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}
