import { useState, type FormEvent } from 'react';
import { call, type UiState } from '../ui';

export function Unlock({ state, onUnlocked }: { state: UiState; onUnlocked: (s: UiState) => void }) {
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = state.pending[0];

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onUnlocked(await call<UiState>('unlock', { password }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="screen center" onSubmit={submit}>
      <h1>Keel Wallet</h1>
      {pending && <p className="notice"><strong>{pending.origin}</strong> is waiting for your approval. Unlock to review the request.</p>}
      <label>Password<input type="password" value={password} onChange={(e) => setPassword(e.target.value)} autoFocus autoComplete="current-password" /></label>
      {error && <p className="error">{error}</p>}
      <button className="primary" type="submit" disabled={busy || !password}>{busy ? 'Unlocking…' : 'Unlock'}</button>
    </form>
  );
}
