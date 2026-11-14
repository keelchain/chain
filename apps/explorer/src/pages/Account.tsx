import { useState } from 'react';
import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { depositProgress, externalNetworks, externalAddressUrl, fetchIncoming, isProcessing, stageLabel, type DepositProgress, type ExternalChain } from '@keelchain/sdk';
import { useAccount, useAccountDeposits, useAccountOffers, useAccountOrders, useAccountTrades, useAccountTransfers, useAccountTxs, useParams as useChainParams } from '../api/hooks';
import { OffersTable, TradesTable, TransfersTable, TxsTable } from '../components/tables';
import { Amount, Badge, BlockLink, Card, CopyButton, ErrorState, ExternalLink, Hex, KV, Loading, Meter, Pager, StatTile, StatusBadge, Tabs, TimeAgo, TxLink, PairLink, Price } from '../components/ui';
import { formatInt, formatPercent, splitPair } from '../lib/format';
import { chainName } from '../lib/links';
import { useHref, useNetwork } from '../network/NetworkContext';

type Tab = 'txs' | 'transfers' | 'deposits' | 'orders' | 'offers' | 'trades';

const ACCOUNT_TYPE_HELP: Record<string, string> = {
  available: 'Spendable balance',
  order_lock: 'Locked under resting orders',
  escrow: 'Locked in P2P trade escrow',
  stake_bond: 'Validator bond',
  observer_bond: 'Observer-signer bond',
  arbitrator_bond: 'Arbitrator bond',
  custody: 'Held in a client-owned vault (the client keeps the keys; see its /v1/custody page)',
  custody_escrow: 'Leaving a client-owned vault: withdrawal in flight',
  vault_asset: 'Reserve this address custodies for its custody balances',
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
            { id: 'deposits', label: 'Deposits' },
            { id: 'orders', label: 'Orders' },
            { id: 'offers', label: 'Offers' },
            { id: 'trades', label: 'Trades' },
          ]}
          value={tab}
          onChange={setTab}
        />
        {tab === 'txs' && <AccountTxs address={address} />}
        {tab === 'transfers' && <AccountTransfers address={address} />}
        {tab === 'deposits' && <AccountDeposits address={address} depositAddresses={a.deposit_addresses} />}
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

const REQUIRED_PARAM: Record<string, string> = { BTC: 'confirmations_btc', ETH: 'confirmations_eth', TRON: 'confirmations_tron' };
const STAGE_TONE: Record<DepositProgress['stage'], 'good' | 'warn' | 'bad' | 'neutral' | 'accent'> = {
  seen: 'warn',
  confirming: 'warn',
  observing: 'accent',
  held: 'accent',
  credited: 'good',
  rejected: 'bad',
};

/**
 * Deposits in flight and landed, per external chain: the deposit address with
 * the network it lives on, what that network shows arriving (looked up on its
 * public API from the browser), and the chain's own deposit rows.
 */
function AccountDeposits({ address, depositAddresses }: { address: string; depositAddresses: { chain: string; index: number; address: string | null }[] }) {
  const { network } = useNetwork();
  const nets = externalNetworks(network.id);
  const params = useChainParams();
  const seen = new Set<string>();
  const rows: { chain: string; index: number; address: string }[] = [];
  for (const d of depositAddresses) {
    if (!d.address || seen.has(d.chain)) continue;
    seen.add(d.chain);
    rows.push({ chain: d.chain, index: d.index, address: d.address });
  }
  if (!rows.length) {
    return (
      <div className="card-body stack">
        <div className="empty">This account has no deposit address yet. One is assigned by a signed RequestDepositAddress, which Keel Wallet sends from its Receive card.</div>
      </div>
    );
  }
  return (
    <div className="card-body stack">
      <div className="notice info small">
        Deposits arrive on {rows.map((r) => nets[r.chain as ExternalChain]?.name ?? r.chain).join(' and ')}. Coins sent on another network, such as Bitcoin testnet3 or Tron Shasta, never reach these addresses and do not appear here.
      </div>
      {rows.map((r) => {
        const required = Number(params.data?.params[REQUIRED_PARAM[r.chain] ?? ''] ?? 0) || (r.chain === 'BTC' ? 2 : r.chain === 'TRON' ? 19 : 12);
        return <ChainDeposits key={r.chain} owner={address} chain={r.chain} depositAddress={r.address} index={r.index} net={nets[r.chain as ExternalChain]} required={required} />;
      })}
    </div>
  );
}

function ChainDeposits({ owner, chain, depositAddress, index, net, required }: { owner: string; chain: string; depositAddress: string; index: number; net: ReturnType<typeof externalNetworks>[ExternalChain] | undefined; required: number }) {
  const onChain = useAccountDeposits(owner, chain);
  const incoming = useQuery({
    queryKey: ['incoming', net?.name, depositAddress],
    queryFn: () => (net ? fetchIncoming(net, depositAddress) : Promise.resolve(null)),
    enabled: !!net?.api,
    refetchInterval: 30_000,
    retry: 1,
  });
  const list = depositProgress(incoming.data ?? null, onChain.data?.deposits ?? [], required);
  const processing = list.filter((p) => isProcessing(p.stage)).length;
  return (
    <Card
      title={
        <span>
          {net?.name ?? chainName(chain)}
          {processing > 0 && <> <Badge tone="warn">{processing} processing</Badge></>}
        </span>
      }
      flush
    >
      <div className="card-body stack">
        <KV
          rows={[
            ['Deposit address', <span className="hex" title={depositAddress}>{net ? <a href={externalAddressUrl(net, depositAddress)} target="_blank" rel="noreferrer noopener">{depositAddress} ↗</a> : depositAddress}<CopyButton text={depositAddress} /></span>],
            ['Index', formatInt(index)],
            ['Credited after', `${required} confirmations on ${net?.name ?? chainName(chain)}`],
          ]}
        />
        {incoming.isError && <div className="notice small">Could not reach {net?.name} to look for incoming transactions. The chain's own records below are unaffected.</div>}
        {net && !net.api && <div className="muted small">Incoming transactions on {net.name} are not looked up; the chain's records appear once the observers report them.</div>}
      </div>
      {onChain.isLoading || (incoming.isLoading && incoming.fetchStatus !== 'idle') ? (
        <Loading rows={2} />
      ) : list.length === 0 ? (
        <div className="empty">Nothing incoming and nothing credited yet.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>External tx</th>
                <th>Asset</th>
                <th className="num">Amount</th>
                <th>Stage</th>
                <th>Seen</th>
              </tr>
            </thead>
            <tbody>
              {list.map((p) => (
                <tr key={p.tx_hash}>
                  <td><ExternalLink chain={p.chain} value={p.tx_hash} id={p.external_tx} what="tx" /></td>
                  <td>{p.asset}</td>
                  <td className="num"><Amount value={p.amount} asset={p.asset} /></td>
                  <td>
                    <Badge tone={STAGE_TONE[p.stage]}>{p.stage}</Badge> <span className="small text-2">{stageLabel(p)}</span>
                  </td>
                  <td>{p.time ? <TimeAgo ts={p.time * 1000} /> : <span className="muted">—</span>}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
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
