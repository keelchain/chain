import { Link, useParams } from 'react-router-dom';
import { useAsset, useAssets, useMarkets } from '../api/hooks';
import { AddressLink, Amount, Badge, Card, ErrorState, KV, Loading, Meter, PairLink, Price, StatTile } from '../components/ui';
import { formatAmount, formatInt, formatPercent } from '../lib/format';
import { chainName } from '../lib/links';
import { useHref } from '../network/NetworkContext';

function KindBadge({ kind, chain }: { kind: string; chain?: string }) {
  if (kind === 'native') return <Badge tone="accent">Native coin</Badge>;
  if (kind === 'stable') return <Badge tone="good">USD stablecoin</Badge>;
  return <Badge>{chain ? `${chainName(chain)} vault` : 'Vault asset'}</Badge>;
}

export function AssetsPage() {
  const href = useHref();
  const q = useAssets();
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="assets" />;
  const assets = q.data ?? [];
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Assets</h1>
        <span className="sub">KEEL (native), KUSD (1:1 USD stablecoin) and vault assets held on other chains by threshold-signature vaults</span>
      </div>
      <Card flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>Asset</th>
                <th>Kind</th>
                <th className="num">Decimals</th>
                <th className="num">Supply</th>
                <th className="num">Holders</th>
                <th className="num">Reserves</th>
                <th>Coverage</th>
              </tr>
            </thead>
            <tbody>
              {assets.map((a) => {
                const ratio = a.reserves && BigInt(a.supply) > 0n ? Number((BigInt(a.reserves) * 10_000n) / BigInt(a.supply)) / 10_000 : null;
                return (
                  <tr key={a.asset}>
                    <td>
                      <Link to={href(`/assets/${encodeURIComponent(a.asset)}`)}>
                        <strong>{a.asset}</strong>
                      </Link>
                    </td>
                    <td>
                      <KindBadge kind={a.kind} chain={a.chain} />
                    </td>
                    <td className="num">{a.decimals}</td>
                    <td className="num">
                      <Amount value={a.supply} asset={a.asset} decimals={a.decimals} />
                    </td>
                    <td className="num">{formatInt(a.holders)}</td>
                    <td className="num">{a.reserves ? <Amount value={a.reserves} asset={a.asset} decimals={a.decimals} /> : <span className="muted">—</span>}</td>
                    <td style={{ minWidth: 140 }}>
                      {ratio !== null ? (
                        <span className="row">
                          <span style={{ width: 80 }}>
                            <Meter ratio={ratio} tone={ratio < 1 ? 'bad' : undefined} />
                          </span>
                          <span className={ratio < 1 ? 'down' : 'up'}>{formatPercent(ratio, 1)}</span>
                        </span>
                      ) : (
                        <span className="muted">n/a</span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}

export function AssetPage() {
  const { asset = '' } = useParams();
  const q = useAsset(decodeURIComponent(asset));
  const markets = useMarkets();
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="asset" />;
  const a = q.data;
  const supply = BigInt(a.supply);
  const ratio = a.reserves && supply > 0n ? Number((BigInt(a.reserves) * 10_000n) / supply) / 10_000 : null;
  const pairs = (markets.data ?? []).filter((m) => m.base === a.asset || m.quote === a.asset);
  return (
    <div className="stack">
      <div className="page-head">
        <h1>{a.asset}</h1>
        <KindBadge kind={a.kind} chain={a.chain} />
      </div>
      <div className="tiles">
        <StatTile label="Supply" value={<Amount value={a.supply} decimals={a.decimals} compact />} hint={`${formatAmount(a.supply, a.decimals)} ${a.asset}`} />
        <StatTile label="Holders" value={formatInt(a.holders)} />
        <StatTile label="Transfers 24h" value={formatInt(a.transfers_24h)} />
        <StatTile label="Decimals" value={a.decimals} hint={`smallest unit 10^-${a.decimals}`} />
        {a.reserves && <StatTile label="Vault reserves" value={<Amount value={a.reserves} decimals={a.decimals} compact />} hint={ratio !== null ? `${formatPercent(ratio, 2)} of liabilities` : undefined} />}
        {a.kind === 'native' && <StatTile label="Hard cap" value="21B" hint="21,000,000,000 KEEL" />}
      </div>
      <div className="grid-2">
        <Card title="About" flush>
          <KV
            rows={[
              ['Kind', <KindBadge kind={a.kind} chain={a.chain} />],
              ...(a.chain ? [['Home chain', <Link to={`../vaults/${a.chain}`} relative="path">{chainName(a.chain)} vault</Link>] as [string, React.ReactNode]] : []),
              ['Role', a.kind === 'native' ? 'Bond for validators, observers and arbitrators; governance vote; price of extra action budget and of listing an offer; burned from every fee.' : a.kind === 'stable' ? 'Unit of account. Minted 1:1 against vaulted USD tokens under per-asset basket caps; redeemable into any basket asset with available reserve; supply ≤ Σ reserves at every block.' : 'Bridged from its home chain. Credited on observer quorum plus a light-client proof where available; reserves must cover user liabilities at every block boundary or outbounds halt.'],
              ...(ratio !== null ? [['Reserve coverage', <span className="row"><span style={{ width: 160 }}><Meter ratio={ratio} tone={ratio < 1 ? 'bad' : undefined} /></span>{formatPercent(ratio, 2)}</span>] as [string, React.ReactNode]] : []),
            ]}
          />
        </Card>
        <Card title="Markets" flush>
          {pairs.length === 0 ? (
            <div className="empty">No listed pair.</div>
          ) : (
            <div className="table-wrap">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Pair</th>
                    <th className="num">Last</th>
                    <th className="num">24h volume</th>
                  </tr>
                </thead>
                <tbody>
                  {pairs.map((m) => (
                    <tr key={m.pair}>
                      <td>
                        <PairLink pair={m.pair} />
                      </td>
                      <td className="num">
                        <Price value={m.last_price} pair={m.pair} />
                      </td>
                      <td className="num">
                        <Amount value={m.volume_24h_quote} asset={m.quote} compact />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      </div>
      <Card title="Top holders" flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>#</th>
                <th>Address</th>
                <th className="num">Balance</th>
                <th className="num">Share</th>
              </tr>
            </thead>
            <tbody>
              {a.holders_top.map((h, i) => (
                <tr key={h.address}>
                  <td className="muted">{i + 1}</td>
                  <td>
                    <AddressLink address={h.address} full />
                  </td>
                  <td className="num">
                    <Amount value={h.balance} asset={a.asset} decimals={a.decimals} />
                  </td>
                  <td className="num">{supply > 0n ? formatPercent(Number((BigInt(h.balance) * 10_000n) / supply) / 10_000, 2) : '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
    </div>
  );
}
