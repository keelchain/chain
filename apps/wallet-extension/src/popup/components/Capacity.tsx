/** The Home card for action capacity: what the account can send now, the KEEL locked for it, and the lock and unlock controls. */
import { useState } from 'react';
import { call } from '../ui';
import { Address } from './Address';
import { ACTIONS_PER_KEEL_PER_DAY, capacityChangeProblem, needsFirstLock, unlockArrival, type Capacity } from '../../core/capacity';
import { formatAmount, formatDate } from '../../core/format';
import { explorerTxUrl, type Network } from '../../core/networks';
import { toUnits } from '../external';

export function CapacityCard({ network, capacity, spendable, onChanged }: { network: Network; capacity: Capacity; spendable: string | number; onChanged: () => void }) {
  const [amount, setAmount] = useState('1');
  const [busy, setBusy] = useState<'lock' | 'unlock' | null>(null);
  const [result, setResult] = useState<{ txId: string | null; ok: boolean; text: string } | null>(null);

  const change = async (kind: 'lock' | 'unlock') => {
    const units = toUnits(amount, 6);
    let held = 0n;
    try {
      held = BigInt(String(spendable));
    } catch {
      held = 0n;
    }
    const problem = capacityChangeProblem(kind, units === null ? null : BigInt(units), capacity, held);
    if (problem || units === null) {
      setResult({ txId: null, ok: false, text: problem ?? 'Enter an amount in KEEL.' });
      return;
    }
    setBusy(kind);
    setResult(null);
    try {
      const r = await call<{ txId: string; ok: boolean; error: string | null }>('send', { kind: kind === 'lock' ? 'lock-budget' : 'unlock-budget', asset: 'KEEL', to: '', amount: units });
      setResult(r.ok ? { txId: r.txId, ok: true, text: kind === 'lock' ? `Locked ${amount} KEEL.` : `Unlocking ${amount} KEEL. It returns to the available balance after the unlock delay; this card shows when.` } : { txId: r.txId, ok: false, text: `Rejected: ${r.error ?? 'unknown'}` });
      onChanged();
    } catch (e) {
      setResult({ txId: null, ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(null);
    }
  };

  const now = Date.now();
  return (
    <div className="card">
      <div className="label">Action capacity</div>
      {needsFirstLock(capacity) ? (
        <p className="notice small">Actions (transfers, orders, offers, withdrawals) need locked KEEL. 1 KEEL locked gives {ACTIONS_PER_KEEL_PER_DAY} actions a day, and it comes back in full after unlocking.</p>
      ) : (
        <table className="balances">
          <tbody>
            <tr><td>Available now</td><td className="num">{capacity.available.toLocaleString('en-US')} actions</td></tr>
            <tr><td>Locked</td><td className="num">{formatAmount(capacity.locked, 6)} KEEL</td></tr>
            <tr><td>Per day</td><td className="num">{capacity.perDay.toLocaleString('en-US')} actions</td></tr>
          </tbody>
        </table>
      )}
      {capacity.unlocking.map((u, i) => (
        <p key={i} className="muted small">Unlocking <b>{formatAmount(u.amount, 6)} KEEL</b>: {unlockArrival(u.readyAt, now)}{u.readyAt > now ? ` (${formatDate(Math.floor(u.readyAt / 1000))})` : ''}</p>
      ))}
      <div>
        <div className="row">
          <input value={amount} placeholder="Amount in KEEL" title="Amount in KEEL" onChange={(e) => setAmount(e.target.value.trim())} />
          <button disabled={busy !== null} onClick={() => void change('lock')}>{busy === 'lock' ? 'Signing…' : 'Lock'}</button>
          <button className="ghost" disabled={busy !== null || capacity.locked === 0n} onClick={() => void change('unlock')}>{busy === 'unlock' ? 'Signing…' : 'Unlock'}</button>
        </div>
      </div>
      {!needsFirstLock(capacity) && <p className="muted small">Each KEEL locked gives {ACTIONS_PER_KEEL_PER_DAY} actions a day; the pool refills over 24 hours. Unlocked KEEL comes back in full after a delay.</p>}
      {result && (
        <div className={result.ok ? 'small' : 'error'}>
          <div>{result.text}</div>
          {result.txId && (
            <div className="row small">
              <Address value={result.txId} />
              {explorerTxUrl(network, result.txId) && <a href={explorerTxUrl(network, result.txId)} target="_blank" rel="noreferrer">View in explorer ↗</a>}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
