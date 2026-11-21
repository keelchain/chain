/**
 * Sending one chain action: the wallet signs, the node admits, the receipt
 * says whether the chain applied it. Each form keeps its own run state so
 * its receipt shows next to the button that caused it.
 */
import { useQueryClient } from '@tanstack/react-query';
import { useState, type ReactNode } from 'react';
import type { Receipt } from '@keelchain/sdk';
import { explorerUrl } from '../network/networks';
import { useNetwork } from '../network/NetworkContext';
import { useWallet } from '../wallet/WalletContext';
import { Badge } from './ui';

export interface ActionRun {
  run(action: unknown, title: string, description?: string): Promise<void>;
  receipt: Receipt | null;
  busy: boolean;
  error: string | null;
}

export function useAction(): ActionRun {
  const w = useWallet();
  const qc = useQueryClient();
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (action: unknown, title: string, description?: string) => {
    setBusy(true);
    setError(null);
    setReceipt(null);
    try {
      const r = await w.signAndSubmit(action, { title, description });
      setReceipt(r);
      await qc.invalidateQueries();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return { run, receipt, busy, error };
}

/** The outcome of the last run: applied or refused with the chain's reason, linked to the explorer. */
export function ActionResult({ act }: { act: ActionRun }) {
  const { network } = useNetwork();
  if (act.error) {
    return (
      <div className="receipt bad" role="alert">
        <Badge tone="bad">Not sent</Badge> <span className="small">{act.error}</span>
      </div>
    );
  }
  const r = act.receipt;
  if (!r) return null;
  return (
    <div className={`receipt ${r.ok ? 'ok' : 'bad'}`} role="status">
      {r.ok ? <Badge tone="good">Applied</Badge> : <Badge tone="bad">Refused</Badge>}
      <a href={explorerUrl(network.id, `/tx/${r.tx_id}`)} target="_blank" rel="noreferrer noopener" className="mono small">
        {r.tx_id.slice(0, 16)}… ↗
      </a>
      {!r.ok && r.error && <span className="small">{r.error.message}</span>}
    </div>
  );
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="small text-2">{hint}</span>}
    </label>
  );
}

/** The submit button of a form that sends one action. */
export function Submit({ act, children, disabled = false }: { act: ActionRun; children: ReactNode; disabled?: boolean }) {
  return (
    <button type="submit" className="btn primary" disabled={act.busy || disabled}>
      {act.busy ? 'Waiting for the wallet…' : children}
    </button>
  );
}
