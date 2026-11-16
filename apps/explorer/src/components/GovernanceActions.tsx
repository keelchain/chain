import { useQueryClient } from '@tanstack/react-query';
import { useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import type { Receipt } from '@keelchain/sdk';
import { useParams as useChainParams } from '../api/hooks';
import type { Proposal } from '../api/types';
import { useHref } from '../network/NetworkContext';
import { useWallet } from '../wallet/WalletContext';
import { Badge, Card } from './ui';

/** Governance is the only way a parameter changes: there is no admin key.
 *  These controls put a ParamChange proposal on chain, vote on an open one
 *  and execute a passed one, each signed in Keel Wallet. */
function useGovAction() {
  const w = useWallet();
  const qc = useQueryClient();
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
  return { w, run, busy, error, receipt };
}

function ReceiptLine({ r }: { r: Receipt | null }) {
  const href = useHref();
  if (!r) return null;
  return (
    <div className={`receipt ${r.ok ? 'ok' : 'bad'}`} role="status">
      {r.ok ? <Badge tone="good">Applied</Badge> : <Badge tone="bad">Refused</Badge>}{' '}
      <Link to={href(`/tx/${r.tx_id}`)} className="mono small">
        {r.tx_id.slice(0, 16)}…
      </Link>
      {!r.ok && r.error && <span className="small text-2"> {r.error.message}</span>}
    </div>
  );
}

function ConnectPrompt({ what }: { what: string }) {
  const w = useWallet();
  return (
    <div className="row">
      <button type="button" className="btn primary" onClick={() => void w.connect()} disabled={w.busy}>
        {w.busy ? 'Connecting…' : `Connect Keel Wallet to ${what}`}
      </button>
      {w.error && <span className="small down">{w.error}</span>}
    </div>
  );
}

/** Flatten `/v1/params` one level into the dotted keys `ParamChange` takes. */
function paramKeys(params: unknown): string[] {
  if (!params || typeof params !== 'object') return [];
  const out: string[] = [];
  for (const [k, v] of Object.entries(params as Record<string, unknown>)) {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      for (const inner of Object.keys(v as Record<string, unknown>)) out.push(`${k}.${inner}`);
    } else {
      out.push(k);
    }
  }
  return out.sort();
}

export function ProposeParamCard() {
  const { w, run, busy, error, receipt } = useGovAction();
  const params = useChainParams();
  const [key, setKey] = useState('');
  const [value, setValue] = useState('');
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const keys = paramKeys(params.data);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!/^\d+$/.test(value.trim())) return;
    void run(
      { Propose: { title: title.trim() || `Set ${key.trim()}`, description: description.trim(), kind: { ParamChange: { key: key.trim(), value: BigInt(value.trim()) } } } },
      `Propose: set ${key.trim()} to ${value.trim()}`,
    );
  };
  return (
    <Card title={<h2>Propose a parameter change</h2>}>
      <p className="text-2 small">
        There is no administrative key: every parameter moves by vote. A proposal locks the proposal deposit in KEEL (returned unless vetoed), is voted on by bonded KEEL for the voting period, and anyone can execute it after the timelock. Values are in the parameter's smallest units; the <Link to={useHref()('/governance/params')}>parameters page</Link> shows current values and keelchain.com/docs/governance explains each key.
      </p>
      {!w.account ? (
        <ConnectPrompt what="propose" />
      ) : (
        <form className="form" onSubmit={submit}>
          <label className="field">
            <span className="field-label">Parameter</span>
            <input className="input mono" list="param-keys" value={key} onChange={(e) => setKey(e.target.value)} placeholder="budget.base" required />
            <datalist id="param-keys">
              {keys.map((k) => (
                <option key={k} value={k} />
              ))}
            </datalist>
          </label>
          <label className="field">
            <span className="field-label">New value</span>
            <input className="input" value={value} onChange={(e) => setValue(e.target.value)} inputMode="numeric" pattern="[0-9]+" required />
          </label>
          <label className="field">
            <span className="field-label">Title</span>
            <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder={key ? `Set ${key}` : 'Why this change'} />
          </label>
          <label className="field">
            <span className="field-label">Description</span>
            <textarea className="input" value={description} onChange={(e) => setDescription(e.target.value)} rows={3} />
          </label>
          <div className="row">
            <button type="submit" className="btn primary" disabled={busy}>
              {busy ? 'Waiting for the wallet…' : 'Propose'}
            </button>
            {error && <span className="small down">{error}</span>}
          </div>
          <ReceiptLine r={receipt} />
        </form>
      )}
    </Card>
  );
}

const CHOICES: { choice: string; label: string; tone: 'good' | 'warn' | 'neutral' | 'bad' }[] = [
  { choice: 'Yes', label: 'Yes', tone: 'good' },
  { choice: 'No', label: 'No', tone: 'warn' },
  { choice: 'Abstain', label: 'Abstain', tone: 'neutral' },
  { choice: 'Veto', label: 'No with veto', tone: 'bad' },
];

export function ProposalActions({ p }: { p: Proposal }) {
  const { w, run, busy, error, receipt } = useGovAction();
  const canVote = p.status === 'voting';
  const canExecute = p.status === 'passed';
  if (!canVote && !canExecute) return null;
  return (
    <Card title={<h2>{canVote ? 'Vote' : 'Execute'}</h2>}>
      <p className="text-2 small">
        {canVote
          ? 'Your vote weighs your bonded KEEL plus delegations received; a later vote replaces an earlier one. Unbonded KEEL does not vote.'
          : 'The proposal passed. Once its timelock is over, anyone may execute it; execution applies the change in that block.'}
      </p>
      {!w.account ? (
        <ConnectPrompt what={canVote ? 'vote' : 'execute'} />
      ) : (
        <>
          <div className="row">
            {canVote &&
              CHOICES.map((c) => (
                <button key={c.choice} type="button" className="btn" disabled={busy} onClick={() => void run({ Vote: { proposal_id: p.id, choice: c.choice } }, `Vote ${c.label} on proposal #${p.id}`)}>
                  {c.label}
                </button>
              ))}
            {canExecute && (
              <button type="button" className="btn primary" disabled={busy} onClick={() => void run({ ExecuteProposal: { proposal_id: p.id } }, `Execute proposal #${p.id}`)}>
                Execute
              </button>
            )}
            {error && <span className="small down">{error}</span>}
          </div>
          <ReceiptLine r={receipt} />
        </>
      )}
    </Card>
  );
}
