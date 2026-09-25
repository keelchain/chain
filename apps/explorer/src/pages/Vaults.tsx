import { Link, useParams, useSearchParams } from 'react-router-dom';
import { useLightning, useOutbounds, useVaultDeposits, useVaults } from '../api/hooks';
import type { LightningStatus, Vault } from '../api/types';
import { AddressLink, Amount, Badge, BlockLink, Card, ErrorState, ExternalLink, KV, Loading, Meter, StatusBadge, Tabs } from '../components/ui';
import { formatBps, formatInt, formatPercent, truncateMiddle } from '../lib/format';
import { chainName } from '../lib/links';
import { useHref } from '../network/NetworkContext';

function coverage(v: Vault, asset: string): number | null {
  const r = v.reserves.find((x) => x.asset === asset)?.amount;
  const l = v.liabilities.find((x) => x.asset === asset)?.amount;
  if (!r || !l || BigInt(l) === 0n) return null;
  return Number((BigInt(r) * 10_000n) / BigInt(l)) / 10_000;
}

function VaultCard({ v }: { v: Vault }) {
  const href = useHref();
  return (
    <Card
      title={
        <h2>
          <Link to={href(`/vaults/${v.chain}`)}>{chainName(v.chain)} vault</Link>
        </h2>
      }
      actions={v.halted ? <Badge tone="bad">Outbounds halted</Badge> : <Badge tone="good">Healthy</Badge>}
      flush
    >
      <div className="table-wrap">
        <table className="tbl">
          <thead>
            <tr>
              <th>Asset</th>
              <th className="num">Reserves</th>
              <th className="num">Liabilities</th>
              <th>Coverage</th>
            </tr>
          </thead>
          <tbody>
            {v.reserves.map((r) => {
              const c = coverage(v, r.asset);
              const l = v.liabilities.find((x) => x.asset === r.asset)?.amount;
              return (
                <tr key={r.asset}>
                  <td>
                    <Link to={href(`/assets/${encodeURIComponent(r.asset)}`)}>{r.asset}</Link>
                  </td>
                  <td className="num">
                    <Amount value={r.amount} asset={r.asset} />
                  </td>
                  <td className="num">{l ? <Amount value={l} asset={r.asset} /> : '—'}</td>
                  <td style={{ minWidth: 150 }}>
                    {c !== null ? (
                      <span className="row">
                        <span style={{ width: 80 }}>
                          <Meter ratio={c} tone={c < 1 ? 'bad' : undefined} />
                        </span>
                        <span className={c < 1 ? 'down' : 'up'}>{formatPercent(c, 2)}</span>
                      </span>
                    ) : (
                      '—'
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <div className="card-body small text-2">
        Epoch {v.epoch} · {formatInt(v.address_count)} deposit addresses · fee rate {v.fee_rate} {v.chain === 'BTC' ? 'sat/vB' : v.chain === 'ETH' ? 'gwei' : 'sun/bandwidth'}{v.threshold && v.signers ? ` · ${v.threshold}-of-${v.signers.length} threshold signature` : ''}
      </div>
    </Card>
  );
}

export function VaultsPage() {
  const q = useVaults();
  const [sp, setSp] = useSearchParams();
  const tab = sp.get('tab') === 'outbounds' ? 'outbounds' : 'vaults';
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="vaults" />;
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Vaults</h1>
        <span className="sub">Threshold-signature custody per chain; at every block reserves must cover user liabilities or payouts of that asset halt</span>
      </div>
      <Tabs
        tabs={[
          { id: 'vaults', label: 'Reserves' },
          { id: 'outbounds', label: 'Outbounds and batches' },
        ]}
        value={tab}
        onChange={(t) => setSp(t === 'vaults' ? {} : { tab: t })}
      />
      {tab === 'vaults' ? (
        <div className="stack">
          {(q.data ?? []).map((v) => (
            <VaultCard key={v.chain} v={v} />
          ))}
          <LightningCard />
        </div>
      ) : (
        <OutboundsView />
      )}
    </div>
  );
}

const BTC = 'BTC.BTC';

function sumSats(values: string[]): string {
  return values.reduce((acc, v) => (/^\d+$/.test(v) ? acc + BigInt(v) : acc), 0n).toString();
}

/**
 * Bitcoin Lightning (2026-09-10): observer-run nodes hold bounded
 * pools of the vault's BTC; deposits credit through them and payouts are
 * assigned to one observer with a routing-fee allowance, refunded at the
 * deadline if unpaid. Lives under the vault cards because it is the BTC
 * vault's fast lane, not a vault of its own.
 */
function LightningCard() {
  const q = useLightning();
  if (q.isLoading) return <Card title="Lightning (BTC)"><Loading rows={3} /></Card>;
  if (q.isError) return <Card title="Lightning (BTC)"><ErrorState error={q.error} what="Lightning" /></Card>;
  const d: LightningStatus = q.data ?? { enabled: false, params: { pool_cap_sats: '0', max_deposit_sats: '0', max_withdraw_sats: '0', max_fee_bps: 0, min_fee_sats: '0', payout_timeout_blocks: 0, daily_cap_sats: '0' }, pool_total: '0', pools: [], assignments: [], sweeps: [] };
  const available = sumSats(d.pools.map((p) => p.available));
  return (
    <Card title="Lightning (BTC)" actions={d.enabled ? <Badge tone="good">Enabled</Badge> : <Badge tone="bad">Disabled</Badge>} flush>
      <KV
        rows={[
          ['Total pool', <Amount value={d.pool_total} asset={BTC} />],
          ['Observers', `${formatInt(d.pools.length)} with a pool`],
          ['Available liquidity', <Amount value={available} asset={BTC} />],
          ['Pool cap per observer', <Amount value={d.params.pool_cap_sats} asset={BTC} />],
          ['Per invoice', <span>deposit up to <Amount value={d.params.max_deposit_sats} asset={BTC} />, payout up to <Amount value={d.params.max_withdraw_sats} asset={BTC} /></span>],
          ['Routing fee allowance', <span>{formatBps(d.params.max_fee_bps)} of the amount, at least <Amount value={d.params.min_fee_sats} asset={BTC} /></span>],
          ['Payout timeout', `${formatInt(d.params.payout_timeout_blocks)} blocks`],
          ['Daily cap per observer', d.params.daily_cap_sats === '0' ? 'none' : <Amount value={d.params.daily_cap_sats} asset={BTC} />],
        ]}
      />
      <div className="card-head">
        <h3>Pools ({d.pools.length})</h3>
      </div>
      {d.pools.length === 0 ? (
        <div className="empty">No Lightning pools yet.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl" data-testid="lightning-pools">
            <thead>
              <tr>
                <th>Observer</th>
                <th>Node</th>
                <th className="num">Balance</th>
                <th className="num">Available</th>
                <th className="num">Pending out</th>
                <th className="num">Credited today</th>
                <th>Registered</th>
              </tr>
            </thead>
            <tbody>
              {d.pools.map((p) => (
                <tr key={p.observer}>
                  <td>
                    <AddressLink address={p.observer} copy={false} />
                  </td>
                  <td>
                    <span className="hex" title={p.node_id}>{truncateMiddle(p.node_id, 8, 6)}</span>
                  </td>
                  <td className="num">
                    <Amount value={p.balance} asset={BTC} />
                  </td>
                  <td className="num">
                    <Amount value={p.available} asset={BTC} />
                  </td>
                  <td className="num">
                    <Amount value={p.pending_out} asset={BTC} />
                  </td>
                  <td className="num">
                    <Amount value={p.credited_today} asset={BTC} />
                  </td>
                  <td>
                    <BlockLink height={p.registered_height} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="card-head">
        <h3>Open payouts ({d.assignments.length})</h3>
      </div>
      {d.assignments.length === 0 ? (
        <div className="empty">No payout is waiting on an observer.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl" data-testid="lightning-payouts">
            <thead>
              <tr>
                <th>Outbound</th>
                <th>Owner</th>
                <th>Observer</th>
                <th className="num">Amount</th>
                <th className="num">Fee allowance</th>
                <th>Deadline</th>
              </tr>
            </thead>
            <tbody>
              {d.assignments.map((a) => (
                <tr key={a.outbound_id}>
                  <td>#{a.outbound_id}</td>
                  <td>
                    <AddressLink address={a.owner} copy={false} />
                  </td>
                  <td>
                    <AddressLink address={a.observer} copy={false} />
                  </td>
                  <td className="num">
                    <Amount value={a.amount} asset={BTC} />
                  </td>
                  <td className="num">
                    <Amount value={a.fee_allowance} asset={BTC} />
                  </td>
                  <td>
                    <BlockLink height={a.deadline_height} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="card-head">
        <h3>Pending sweeps ({d.sweeps.length})</h3>
      </div>
      {d.sweeps.length === 0 ? (
        <div className="empty">No excess is being swept back to the vault.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl" data-testid="lightning-sweeps">
            <thead>
              <tr>
                <th>Bitcoin tx</th>
                <th>Observer</th>
                <th className="num">Amount</th>
              </tr>
            </thead>
            <tbody>
              {d.sweeps.map((sw) => (
                <tr key={`${sw.tx_hash}-${sw.observer}`}>
                  <td>
                    <ExternalLink chain="BTC" value={sw.tx_hash} what="tx" />
                  </td>
                  <td>
                    <AddressLink address={sw.observer} copy={false} />
                  </td>
                  <td className="num">
                    <Amount value={sw.amount} asset={BTC} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <div className="card-body small text-2">Observers run the Lightning nodes; the chain caps what each may hold, assigns every payout to one observer with a routing-fee allowance (the unspent part goes back to the user) and refunds the user if the invoice is not paid by the deadline. Excess over the cap is swept back to the on-chain vault.</div>
    </Card>
  );
}

function OutboundsView() {
  const [sp, setSp] = useSearchParams();
  const status = sp.get('status') ?? '';
  const q = useOutbounds(status || undefined);
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="outbounds" />;
  const { outbounds, batches } = q.data ?? { outbounds: [], batches: [] };
  return (
    <div className="stack">
      <div className="filters">
        <select className="select" aria-label="Outbound status" value={status} onChange={(e) => { const n = new URLSearchParams(sp); if (e.target.value) n.set('status', e.target.value); else n.delete('status'); setSp(n); }}>
          <option value="">Any status</option>
          <option value="queued">Queued</option>
          <option value="batched">Batched</option>
          <option value="confirmed">Confirmed</option>
          <option value="failed">Failed</option>
        </select>
      </div>
      <Card title={`Outbounds (${outbounds.length})`} flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>#</th>
                <th>Chain</th>
                <th>Owner</th>
                <th className="num">Amount</th>
                <th className="num">Network fee</th>
                <th>Destination</th>
                <th>Batch</th>
                <th>External tx</th>
                <th>Queued</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {outbounds.map((o) => (
                <tr key={o.id}>
                  <td>{o.id}</td>
                  <td>{chainName(o.chain)}</td>
                  <td>
                    <AddressLink address={o.owner} copy={false} />
                  </td>
                  <td className="num">
                    <Amount value={o.amount} asset={o.asset} />
                  </td>
                  <td className="num">{o.fee !== '0' ? <Amount value={o.fee} asset={o.chain === 'BTC' ? 'BTC.BTC' : o.chain === 'ETH' ? 'ETH.ETH' : 'TRON.TRX'} /> : <span className="muted">pending</span>}</td>
                  <td>
                    <ExternalLink chain={o.chain} value={o.to} what="address" />
                  </td>
                  <td>{o.batch_id ? `#${o.batch_id}` : <span className="muted">—</span>}</td>
                  <td>{o.tx_hash ? <ExternalLink chain={o.chain} value={o.tx_hash} what="tx" /> : <span className="muted">not broadcast</span>}</td>
                  <td>
                    <BlockLink height={o.queued_height} />
                  </td>
                  <td>
                    <StatusBadge status={o.status} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
      <Card title={`Batches (${batches.length})`} flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>#</th>
                <th>Chain</th>
                <th className="num">Outbounds</th>
                <th>External tx</th>
                <th className="num">Fee paid</th>
                <th>Created</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {batches.map((b) => (
                <tr key={b.id}>
                  <td>{b.id}</td>
                  <td>{chainName(b.chain)}</td>
                  <td className="num">{b.outbound_ids.length}</td>
                  <td>{b.tx_hash ? <ExternalLink chain={b.chain} value={b.tx_hash} what="tx" /> : <span className="muted">signing</span>}</td>
                  <td className="num">{b.fee_paid ? <Amount value={b.fee_paid} decimals={8} /> : '—'}</td>
                  <td>
                    <BlockLink height={b.created_height} />
                  </td>
                  <td>
                    <StatusBadge status={b.status} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="card-body small text-2">Withdrawals are batched per chain, signed only for withdrawal sets finalized at a height, broadcast by observers and observed back in like deposits. The fee is the median of observer estimates, passed through to the withdrawing users plus a flat fee.</div>
      </Card>
    </div>
  );
}

export function VaultPage() {
  const { chain = '' } = useParams();
  const [sp, setSp] = useSearchParams();
  const status = sp.get('status') ?? '';
  const vaults = useVaults();
  const q = useVaultDeposits(chain, status || undefined);
  const v = vaults.data?.find((x) => x.chain === chain);
  if (vaults.isLoading || q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="vault" />;
  const deposits = q.data?.deposits ?? [];
  return (
    <div className="stack">
      <div className="page-head">
        <h1>{chainName(chain)} vault</h1>
        {v && (v.halted ? <Badge tone="bad">Outbounds halted</Badge> : <Badge tone="good">Healthy</Badge>)}
      </div>
      {v && (
        <div className="grid-2">
          <VaultCard v={v} />
          <Card title="Key and signers" flush>
            <KV
              rows={[
                ['Epoch', formatInt(v.epoch)],
                ['Threshold', v.threshold && v.signers ? `${v.threshold} of ${v.signers.length} observer-signers` : '—'],
                ['Signers', v.signers ? <ul style={{ margin: 0, paddingLeft: 18 }}>{v.signers.map((s) => <li key={s}><AddressLink address={s} /></li>)}</ul> : '—'],
                ['Credit rule', chain === 'TRON' ? 'Attestation quorum only (no practical light client); higher bonds, lower caps.' : `Observer attestation quorum plus a ${chain === 'BTC' ? 'SPV header + merkle' : 'sync-committee finalized receipt'} proof verified by every validator.`],
                ['Key derivation', 'One threshold master key per observer epoch; per-user deposit addresses are non-hardened children. Sets change per epoch with a fresh key generation and a migration of balances.'],
              ]}
            />
          </Card>
        </div>
      )}
      <div className="filters">
        <select className="select" aria-label="Deposit status" value={status} onChange={(e) => { const n = new URLSearchParams(sp); if (e.target.value) n.set('status', e.target.value); else n.delete('status'); setSp(n); }}>
          <option value="">Any status</option>
          <option value="pending">Pending</option>
          <option value="credited">Credited</option>
          <option value="held">Held</option>
          <option value="rejected">Rejected</option>
        </select>
      </div>
      <Card title={`Deposits (${deposits.length})`} flush>
        {deposits.length === 0 ? (
          <div className="empty">No deposits match.</div>
        ) : (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>External tx</th>
                  <th>Asset</th>
                  <th className="num">Amount</th>
                  <th>Owner</th>
                  <th className="num">Depth</th>
                  <th className="num">Votes</th>
                  <th>Observed</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {deposits.map((d) => (
                  <tr key={`${d.tx_hash}-${d.index}`}>
                    <td>
                      <ExternalLink chain={d.chain} value={d.tx_hash} what="tx" />
                    </td>
                    <td>{d.asset}</td>
                    <td className="num">
                      <Amount value={d.amount} asset={d.asset} />
                    </td>
                    <td>
                      <AddressLink address={d.owner} copy={false} />
                    </td>
                    <td className={`num ${d.depth < d.required_depth ? 'text-2' : ''}`}>
                      {d.depth} / {d.required_depth}
                    </td>
                    <td className="num">{d.votes}</td>
                    <td>
                      <BlockLink height={d.last_height} />
                    </td>
                    <td>
                      <StatusBadge status={d.status} />
                      {d.status === 'held' && d.release_height && (
                        <span className="muted small">
                          {' '}
                          until <BlockLink height={d.release_height} />
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
