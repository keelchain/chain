import { useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useAccount, useAccountOffers, useAccountOrders, useAccountTrades, useAccountTransfers, useAccountTxs } from '../api/hooks';
import { OffersTable, TradesTable, TransfersTable, TxsTable } from '../components/tables';
import { Amount, Badge, BlockLink, Card, ErrorState, ExternalLink, Hex, KV, Loading, Meter, Pager, StatTile, StatusBadge, Tabs, TxLink, PairLink, Price } from '../components/ui';
import { formatInt, formatPercent, splitPair } from '../lib/format';
import { chainName } from '../lib/links';
import { useHref } from '../network/NetworkContext';

type Tab = 'txs' | 'transfers' | 'orders' | 'offers' | 'trades';

const ACCOUNT_TYPE_HELP: Record<string, string> = {
  available: 'Spendable balance',
  order_lock: 'Locked under resting orders',
  escrow: 'Locked in P2P trade escrow',
  stake_bond: 'Validator bond',
  observer_bond: 'Observer-signer bond',
  arbitrator_bond: 'Arbitrator bond',
  offer_deposit: 'Refundable offer deposits',
  delegation: 'Delegated to validators',
  unbonding: 'Unbonding (time-locked)',
  held: 'Large deposit, delayed availability',
};

export function AccountPage() {
  const { addr = '' } = useParams();
  const address = addr.toLowerCase().replace(/^0x/, '');
  const href = useHref();
  const [sp, setSp] = useSearchParams();
  const tab = (sp.get('tab') as Tab) || 'txs';
  const setTab = (t: Tab) => {
    const next = new URLSearchParams(sp);
    next.set('tab', t);
    setSp(next, { replace: true });
  };
  const q = useAccount(address);
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="account" />;
  const a = q.data;
  const budget = typeof a.budget === 'number' ? { remaining: a.budget, limit: a.budget } : a.budget;
  const byAsset = new Map<string, typeof a.balances>();
  for (const b of a.balances) byAsset.set(b.asset, [...(byAsset.get(b.asset) ?? []), b]);

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Account</h1>
        {a.validator && <Badge tone="accent">Validator{a.validator.jailed ? ' · jailed' : ''}</Badge>}
        <span className="sub">
          <Hex value={a.address} full />
        </span>
      </div>
      <div className="tiles">
        <StatTile label="Transactions" value={formatInt(a.tx_count)} hint={<span>first seen at <BlockLink height={a.first_seen_height} /></span>} />
        <StatTile label="Nonce" value={formatInt(a.nonce)} hint="next expected sequence" />
        <StatTile label="Tier" value={a.tier} hint="attested KYC tier (0 = none)" />
        <StatTile label="Action budget" value={formatInt(budget.remaining)} hint={<span>of {formatInt(budget.limit)} · grows with filled volume</span>} />
        <StatTile label="Offers" value={formatInt(a.offers_count)} />
        <StatTile label="Trades" value={formatInt(a.trades_count)} />
      </div>
      <div className="grid-2">
        <Card title="Balances" flush>
          {byAsset.size === 0 ? (
            <div className="empty">No balances.</div>
          ) : (
            <div className="table-wrap">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Asset</th>
                    <th>Account</th>
                    <th className="num">Balance</th>
                  </tr>
                </thead>
                <tbody>
                  {[...byAsset.entries()].map(([asset, rows]) =>
                    rows.map((b, i) => (
                      <tr key={`${asset}-${b.account_type}`}>
                        <td>{i === 0 ? <Link to={href(`/assets/${encodeURIComponent(asset)}`)}>{asset}</Link> : ''}</td>
                        <td>
                          <span title={ACCOUNT_TYPE_HELP[b.account_type]}>{b.account_type.replace(/_/g, ' ')}</span>
                        </td>
                        <td className="num">
                          <Amount value={b.balance} asset={asset} decimals={b.decimals} />
                        </td>
                      </tr>
                    )),
                  )}
                </tbody>
              </table>
            </div>
          )}
        </Card>
        <div className="stack">
          <Card title="Action budget">
            <Meter ratio={budget.limit ? budget.remaining / budget.limit : 0} tone={budget.limit && budget.remaining / budget.limit < 0.1 ? 'warn' : undefined} />
            <div className="small text-2" style={{ marginTop: 8 }}>
              {formatInt(budget.remaining)} of {formatInt(budget.limit)} actions remaining ({formatPercent(budget.limit ? budget.remaining / budget.limit : 0, 0)}). There is no gas: every address starts with a free allowance and earns one action per USD filled on the book; cancels are always allowed up to a wider cap. Extra budget can be bought in KEEL.
            </div>
          </Card>
          <Card title="Deposit addresses" flush>
            {a.deposit_addresses.length === 0 ? (
              <div className="empty">No deposit addresses requested.</div>
            ) : (
              <div className="table-wrap">
                <table className="tbl">
                  <thead>
                    <tr>
                      <th>Chain</th>
                      <th>Address</th>
                      <th className="num">Index</th>
                    </tr>
                  </thead>
                  <tbody>
                    {a.deposit_addresses.map((d) => (
                      <tr key={`${d.chain}-${d.index}`}>
                        <td>
                          <Link to={href(`/vaults/${d.chain}`)}>{chainName(d.chain)}</Link>
                        </td>
                        <td>
                          <ExternalLink chain={d.chain} value={d.address} what="address" />
                        </td>
                        <td className="num">{d.index}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            <div className="card-body small text-2">Non-hardened children of the epoch vault key: no whole key exists anywhere.</div>
          </Card>
          {a.validator && (
            <Card title="Validator" flush>
              <KV
                rows={[
                  ['Consensus key', <Hex value={a.validator.consensus_key} full />],
                  ['Self bond', <Amount value={a.validator.self_bond} asset="KEEL" />],
                  ['Delegated', <Amount value={a.validator.delegated} asset="KEEL" />],
                  ['Voting power', <Amount value={a.validator.power} asset="KEEL" />],
                  ['Uptime', formatPercent(a.validator.uptime ?? null, 2)],
                  ['Status', <StatusBadge status={a.validator.jailed ? 'jailed' : 'active'} />],
                ]}
              />
            </Card>
          )}
        </div>
      </div>
      <Card flush>
        <Tabs
          tabs={[
            { id: 'txs', label: 'Transactions' },
            { id: 'transfers', label: 'Transfers' },
            { id: 'orders', label: 'Orders' },
            { id: 'offers', label: 'Offers' },
            { id: 'trades', label: 'Trades' },
          ]}
          value={tab}
          onChange={setTab}
        />
        {tab === 'txs' && <AccountTxs address={address} />}
        {tab === 'transfers' && <AccountTransfers address={address} />}
        {tab === 'orders' && <AccountOrders address={address} />}
        {tab === 'offers' && <AccountOffers address={address} />}
        {tab === 'trades' && <AccountTrades address={address} />}
      </Card>
    </div>
  );
}

function AccountTxs({ address }: { address: string }) {
  const [cursor, setCursor] = useState<string | undefined>();
  const [acc, setAcc] = useState<import('../api/types').Tx[]>([]);
  const q = useAccountTxs(address, { limit: 25, cursor });
  const rows = [...acc, ...(q.data?.txs ?? [])];
  if (q.isLoading && !rows.length) return <Loading rows={6} />;
  if (q.isError) return <div className="card-body"><ErrorState error={q.error} what="transactions" /></div>;
  return (
    <>
      <TxsTable txs={rows} hideSigner={false} />
      <Pager hasMore={!!q.data?.next_cursor} loading={q.isFetching} onMore={() => { setAcc(rows); setCursor(q.data?.next_cursor ?? undefined); }} />
    </>
  );
}

function AccountTransfers({ address }: { address: string }) {
  const [cursor, setCursor] = useState<string | undefined>();
  const [acc, setAcc] = useState<import('../api/types').Transfer[]>([]);
  const q = useAccountTransfers(address, { limit: 25, cursor });
  const rows = [...acc, ...(q.data?.transfers ?? [])];
  if (q.isLoading && !rows.length) return <Loading rows={6} />;
  if (q.isError) return <div className="card-body"><ErrorState error={q.error} what="transfers" /></div>;
  return (
    <>
      <TransfersTable transfers={rows} self={address} />
      <Pager hasMore={!!q.data?.next_cursor} loading={q.isFetching} onMore={() => { setAcc(rows); setCursor(q.data?.next_cursor ?? undefined); }} />
    </>
  );
}

function AccountOrders({ address }: { address: string }) {
  const href = useHref();
  const q = useAccountOrders(address);
  if (q.isLoading) return <Loading rows={6} />;
  if (q.isError) return <div className="card-body"><ErrorState error={q.error} what="orders" /></div>;
  const orders = q.data ?? [];
  if (!orders.length) return <div className="empty">No orders.</div>;
  return (
    <div className="table-wrap">
      <table className="tbl">
        <thead>
          <tr>
            <th>Order</th>
            <th>Pair</th>
            <th>Side</th>
            <th>Type</th>
            <th className="num">Price</th>
            <th className="num">Quantity</th>
            <th className="num">Filled</th>
            <th>Status</th>
            <th>Created</th>
          </tr>
        </thead>
        <tbody>
          {orders.map((o) => {
            const { base } = splitPair(o.pair);
            return (
              <tr key={o.id}>
                <td>
                  <Link to={href(`/orders/${o.id}`)}>#{o.id}</Link>
                </td>
                <td>
                  <PairLink pair={o.pair} />
                </td>
                <td>
                  <Badge tone={o.side === 'buy' ? 'good' : 'bad'}>{o.side}</Badge>
                </td>
                <td>{o.order_type}</td>
                <td className="num">{o.price ? <Price value={o.price} pair={o.pair} /> : <span className="muted">market</span>}</td>
                <td className="num">
                  <Amount value={o.quantity} asset={base} />
                </td>
                <td className="num">
                  <Amount value={o.filled} asset={base} sym={false} />
                </td>
                <td>
                  <StatusBadge status={o.status} />
                </td>
                <td>
                  <TxLink id={o.tx_id} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function AccountOffers({ address }: { address: string }) {
  const q = useAccountOffers(address);
  if (q.isLoading) return <Loading rows={6} />;
  if (q.isError) return <div className="card-body"><ErrorState error={q.error} what="offers" /></div>;
  return <OffersTable offers={q.data ?? []} hideOwner />;
}

function AccountTrades({ address }: { address: string }) {
  const q = useAccountTrades(address);
  if (q.isLoading) return <Loading rows={6} />;
  if (q.isError) return <div className="card-body"><ErrorState error={q.error} what="trades" /></div>;
  return <TradesTable trades={q.data ?? []} self={address} />;
}
