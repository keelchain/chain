import { Link, useParams } from 'react-router-dom';
import { useParams as useChainParams, useProposal, useProposals } from '../api/hooks';
import type { Proposal, Tally } from '../api/types';
import { FieldValue } from '../components/FieldValue';
import { AddressLink, Amount, Badge, BlockLink, Card, ErrorState, KV, Loading, StatTile, StatusBadge, TxLink } from '../components/ui';
import { decodeAction } from '../lib/decode';
import { formatAmount, formatInt, formatPercent, sentence } from '../lib/format';
import { useHref } from '../network/NetworkContext';

function tallyBars(t: Tally) {
  const yes = BigInt(t.yes);
  const no = BigInt(t.no);
  const abstain = BigInt(t.abstain);
  const veto = BigInt(t.veto);
  const cast = yes + no + abstain + veto;
  const pct = (v: bigint) => (cast > 0n ? Number((v * 10_000n) / cast) / 100 : 0);
  const bonded = t.total_bonded ? BigInt(t.total_bonded) : null;
  const turnout = bonded && bonded > 0n ? Number((cast * 10_000n) / bonded) / 10_000 : null;
  return { yes: pct(yes), no: pct(no), abstain: pct(abstain), veto: pct(veto), cast, turnout };
}

function TallyBar({ tally }: { tally: Tally }) {
  const t = tallyBars(tally);
  return (
    <div>
      <div className="tally" role="img" aria-label={`Yes ${t.yes.toFixed(1)}%, No ${t.no.toFixed(1)}%, Abstain ${t.abstain.toFixed(1)}%, Veto ${t.veto.toFixed(1)}%`}>
        {t.yes > 0 && <span className="yes" style={{ width: `${t.yes}%` }} />}
        {t.no > 0 && <span className="no" style={{ width: `${t.no}%` }} />}
        {t.abstain > 0 && <span className="abstain" style={{ width: `${t.abstain}%` }} />}
        {t.veto > 0 && <span className="veto" style={{ width: `${t.veto}%` }} />}
      </div>
      <div className="legend" style={{ marginTop: 4 }}>
        <span><span className="sw" style={{ background: 'var(--series-3)' }} />Yes {t.yes.toFixed(1)}%</span>
        <span><span className="sw" style={{ background: 'var(--series-2)' }} />No {t.no.toFixed(1)}%</span>
        <span><span className="sw" style={{ background: 'var(--muted)' }} />Abstain {t.abstain.toFixed(1)}%</span>
        <span><span className="sw" style={{ background: 'var(--series-8)' }} />Veto {t.veto.toFixed(1)}%</span>
        {t.turnout !== null && <span className="muted">turnout {formatPercent(t.turnout, 1)}</span>}
      </div>
    </div>
  );
}

function kindName(p: Proposal): string {
  if (typeof p.kind === 'string') return sentence(p.kind);
  const k = Object.keys(p.kind)[0];
  return k ? sentence(k) : 'Proposal';
}

export function GovernancePage() {
  const href = useHref();
  const q = useProposals();
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="proposals" />;
  const list = q.data ?? [];
  const voting = list.filter((p) => p.status === 'voting');
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Governance</h1>
        <span className="sub">Every parameter is a governance parameter: proposals lock a deposit, pass by bonded-KEEL vote with quorum, threshold and veto rules, and execute after a timelock</span>
        <span className="spacer" />
        <Link to={href('/governance/params')} className="btn">
          Parameters
        </Link>
      </div>
      <div className="tiles">
        <StatTile label="Proposals" value={formatInt(list.length)} />
        <StatTile label="In voting" value={formatInt(voting.length)} />
        <StatTile label="Executed" value={formatInt(list.filter((p) => p.status === 'executed').length)} />
        <StatTile label="Rejected or vetoed" value={formatInt(list.filter((p) => p.status === 'rejected' || p.status === 'vetoed').length)} />
      </div>
      <Card flush>
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>#</th>
                <th>Title</th>
                <th>Type</th>
                <th>Proposer</th>
                <th style={{ minWidth: 220 }}>Tally</th>
                <th>Voting ends</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {list.map((p) => {
                const t = tallyBars(p.tally);
                return (
                  <tr key={p.id}>
                    <td>{p.id}</td>
                    <td style={{ whiteSpace: 'normal', minWidth: 240 }}>
                      <Link to={href(`/governance/${p.id}`)}>{p.title}</Link>
                    </td>
                    <td>
                      <Badge>{kindName(p)}</Badge>
                    </td>
                    <td>
                      <AddressLink address={p.proposer} copy={false} />
                    </td>
                    <td>
                      <div className="tally" title={`Yes ${t.yes.toFixed(1)}% · No ${t.no.toFixed(1)}% · Abstain ${t.abstain.toFixed(1)}% · Veto ${t.veto.toFixed(1)}%`}>
                        {t.yes > 0 && <span className="yes" style={{ width: `${t.yes}%` }} />}
                        {t.no > 0 && <span className="no" style={{ width: `${t.no}%` }} />}
                        {t.abstain > 0 && <span className="abstain" style={{ width: `${t.abstain}%` }} />}
                        {t.veto > 0 && <span className="veto" style={{ width: `${t.veto}%` }} />}
                      </div>
                      <span className="small text-2">yes {t.yes.toFixed(0)}% · no {t.no.toFixed(0)}%</span>
                    </td>
                    <td>
                      <BlockLink height={p.voting_end_height} />
                    </td>
                    <td>
                      <StatusBadge status={p.status} />
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

export function ProposalPage() {
  const { id = '' } = useParams();
  const q = useProposal(Number(id));
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="proposal" />;
  const p = q.data;
  const view = decodeAction({ Propose: { title: p.title, description: p.description, kind: p.kind } });
  const kindFields = view.fields.filter((f) => f.name !== 'Title' && f.name !== 'Description');
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Proposal #{p.id}</h1>
        <StatusBadge status={p.status} />
      </div>
      <Card title={p.title}>
        <p style={{ margin: '0 0 12px', whiteSpace: 'pre-wrap' }}>{p.description}</p>
        <TallyBar tally={p.tally} />
      </Card>
      <div className="grid-2">
        <Card title="Details" flush>
          <KV
            rows={[
              ['Proposer', <AddressLink address={p.proposer} full />],
              ['Deposit', <span><Amount value={p.deposit} asset="KEEL" /> <span className="muted">locked; burned only if vetoed</span></span>],
              ['Submitted', <BlockLink height={p.submitted_height} />],
              ['Voting ends', <BlockLink height={p.voting_end_height} />],
              ['Execute after timelock', p.execute_height ? <BlockLink height={p.execute_height} /> : '—'],
              ['Yes', <span><Amount value={p.tally.yes} asset="KEEL" compact /></span>],
              ['No', <span><Amount value={p.tally.no} asset="KEEL" compact /></span>],
              ['Abstain', <span><Amount value={p.tally.abstain} asset="KEEL" compact /></span>],
              ['No with veto', <span><Amount value={p.tally.veto} asset="KEEL" compact /></span>],
              ...(p.tally.total_bonded ? [['Bonded KEEL', <Amount value={p.tally.total_bonded} asset="KEEL" compact />] as [string, React.ReactNode]] : []),
            ]}
          />
        </Card>
        <Card title="What it changes" flush>
          {kindFields.length ? <KV rows={kindFields.map((f) => [f.name, <FieldValue field={f.field} />])} /> : <div className="empty">Text-only proposal (signal).</div>}
        </Card>
      </div>
      <Card title={`Votes (${p.votes.length})`} flush>
        {p.votes.length === 0 ? (
          <div className="empty">No votes cast yet.</div>
        ) : (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>Voter</th>
                  <th>Choice</th>
                  <th className="num">Weight</th>
                  <th>Block</th>
                  <th>Tx</th>
                </tr>
              </thead>
              <tbody>
                {p.votes.map((v) => (
                  <tr key={`${v.voter}-${v.tx_id}`}>
                    <td>
                      <AddressLink address={v.voter} />
                    </td>
                    <td>
                      <Badge tone={v.choice === 'yes' ? 'good' : v.choice === 'veto' ? 'bad' : v.choice === 'no' ? 'warn' : 'neutral'}>{v.choice === 'veto' ? 'no with veto' : v.choice}</Badge>
                    </td>
                    <td className="num">
                      <Amount value={v.weight} asset="KEEL" />
                    </td>
                    <td>
                      <BlockLink height={v.height} />
                    </td>
                    <td>
                      <TxLink id={v.tx_id} />
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

const PARAM_GROUPS: { title: string; match: (k: string) => boolean }[] = [
  { title: 'Fees and splits', match: (k) => /fee|split|referral/.test(k) },
  { title: 'Action budgets', match: (k) => k.startsWith('budget.') || /max_open_orders|house_quote/.test(k) },
  { title: 'P2P offers and disputes', match: (k) => /offer|payment_window|release_grace|ruling|dispute/.test(k) },
  { title: 'Staking and epochs', match: (k) => /bond|unbonding|validators|epoch|observer_reward|slash/.test(k) },
  { title: 'Governance', match: (k) => /proposal|voting|timelock|quorum|threshold|veto/.test(k) },
  { title: 'Vaults and deposits', match: (k) => /confirmations|deposit|outbound|observer_quorum/.test(k) },
];

function paramDisplay(key: string, value: string): string {
  if (/_bps$/.test(key)) return `${formatInt(value)} bps (${(Number(value) / 100).toFixed(2)}%)`;
  if (/_usd_micro$/.test(key)) return `$${formatAmount(value, 6, { maxFraction: 2, minFraction: 2 })}`;
  if (/_secs$/.test(key)) return `${formatInt(value)} s`;
  if (/_blocks$/.test(key)) return `${formatInt(value)} blocks`;
  if (/^(offer_deposit|proposal_deposit|min_.*_bond|budget\.price_per_action)$/.test(key)) return `${formatAmount(value, 6)} KEEL`;
  return formatInt(value);
}

export function ParamsPage() {
  const q = useChainParams();
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError || !q.data) return <ErrorState error={q.error} what="parameters" />;
  const { params, history } = q.data;
  const keys = Object.keys(params).sort();
  const grouped = PARAM_GROUPS.map((g) => ({ ...g, keys: keys.filter(g.match) }));
  const rest = keys.filter((k) => !PARAM_GROUPS.some((g) => g.match(k)));
  if (rest.length) grouped.push({ title: 'Other', match: () => true, keys: rest });
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Chain parameters</h1>
        <span className="sub">Numeric knobs changed by ParamChange proposals, or directly by the revocable parameter admin during launch</span>
      </div>
      <div className="grid-2">
        {grouped.filter((g) => g.keys.length).map((g) => (
          <Card key={g.title} title={g.title} flush>
            <KV rows={g.keys.map((k) => [<code>{k}</code>, <span className="amount">{paramDisplay(k, params[k] ?? '0')}</span>])} />
          </Card>
        ))}
      </div>
      <Card title={`Change history (${history.length})`} flush>
        {history.length === 0 ? (
          <div className="empty">No changes since genesis.</div>
        ) : (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>Block</th>
                  <th>Parameter</th>
                  <th className="num">From</th>
                  <th className="num">To</th>
                  <th>Tx</th>
                </tr>
              </thead>
              <tbody>
                {history.map((h, i) => (
                  <tr key={`${h.tx_id}-${i}`}>
                    <td>
                      <BlockLink height={h.height} />
                    </td>
                    <td>
                      <code>{h.key}</code>
                    </td>
                    <td className="num text-2">{paramDisplay(h.key, h.from)}</td>
                    <td className="num">{paramDisplay(h.key, h.to)}</td>
                    <td>
                      <TxLink id={h.tx_id} />
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
