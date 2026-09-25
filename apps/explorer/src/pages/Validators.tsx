import { useEpochs, useValidators } from '../api/hooks';
import { AddressLink, Amount, BlockLink, Card, ErrorState, Hex, Loading, Meter, StatTile, StatusBadge } from '../components/ui';
import { formatAmount, formatInt, formatPercent } from '../lib/format';

export function ValidatorsPage() {
  const q = useValidators();
  const epochs = useEpochs();
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="validators" />;
  const list = [...(q.data ?? [])].sort((a, b) => (BigInt(b.power) > BigInt(a.power) ? 1 : -1));
  const total = list.reduce((acc, v) => acc + BigInt(v.power), 0n);
  const active = list.filter((v) => !v.jailed);
  const current = epochs.data?.[0];
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Validators</h1>
        <span className="sub">Simplex BFT over the bonded set; round-robin leaders; sets change at epoch boundaries from the staking module</span>
      </div>
      <div className="tiles">
        <StatTile label="Active" value={formatInt(active.length)} hint={`${list.length - active.length} jailed`} />
        <StatTile label="Total power" value={`${formatAmount(total.toString(), 6, { compact: true })} KEEL`} hint="self bond + delegations" />
        <StatTile label="Current epoch" value={current ? formatInt(current.epoch) : '—'} hint={current ? <span>from block <BlockLink height={current.start_height} /></span> : undefined} />
        <StatTile label="Fault tolerance" value={`${Math.floor((active.length - 1) / 3)}`} hint="Byzantine validators tolerated" />
      </div>
      <Card flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>#</th>
                <th>Validator</th>
                <th>Consensus key</th>
                <th className="num">Self bond</th>
                <th className="num">Delegated</th>
                <th className="num">Power</th>
                <th>Share</th>
                <th className="num">Blocks 24h</th>
                <th>Uptime</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {list.map((v, i) => {
                const share = total > 0n ? Number((BigInt(v.power) * 10_000n) / total) / 10_000 : 0;
                return (
                  <tr key={v.address}>
                    <td className="muted">{i + 1}</td>
                    <td>
                      <AddressLink address={v.address} />
                    </td>
                    <td>
                      <Hex value={v.consensus_key} />
                    </td>
                    <td className="num">
                      <Amount value={v.self_bond} asset="KEEL" compact />
                    </td>
                    <td className="num">
                      <Amount value={v.delegated} asset="KEEL" compact />
                    </td>
                    <td className="num">
                      <Amount value={v.power} asset="KEEL" compact />
                    </td>
                    <td style={{ minWidth: 120 }}>
                      <span className="row">
                        <span style={{ width: 60 }}>
                          <Meter ratio={share} />
                        </span>
                        {formatPercent(share, 1)}
                      </span>
                    </td>
                    <td className="num">{v.blocks_proposed_24h !== undefined ? formatInt(v.blocks_proposed_24h) : '—'}</td>
                    <td className={v.uptime !== undefined && v.uptime < 0.95 ? 'down' : ''}>{formatPercent(v.uptime ?? null, 2)}</td>
                    <td>
                      <StatusBadge status={v.jailed ? 'jailed' : 'active'} />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>
      <Card title="Epochs" flush>
        {epochs.isLoading ? (
          <Loading rows={4} />
        ) : (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>Epoch</th>
                  <th>Start height</th>
                  <th className="num">Validators</th>
                </tr>
              </thead>
              <tbody>
                {(epochs.data ?? []).map((e) => (
                  <tr key={e.epoch}>
                    <td>{formatInt(e.epoch)}</td>
                    <td>
                      <BlockLink height={e.start_height} />
                    </td>
                    <td className="num">{e.validators}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="card-body small text-2">Rewards (the validator share of every fee) are distributed at epoch boundaries to observers, validators and delegators pro rata. Observer-signer sets and vault keys also rotate per epoch.</div>
      </Card>
    </div>
  );
}
