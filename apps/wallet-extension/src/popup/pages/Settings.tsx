import { useState } from 'react';
import { call, type UiState } from '../ui';

export function Settings({ state, onState }: { state: UiState; onState: (s: UiState) => void }) {
  const [error, setError] = useState<string | null>(null);
  const [minutes, setMinutes] = useState(String(state.settings.autoLockMinutes));
  const [password, setPassword] = useState('');
  const [mnemonic, setMnemonic] = useState<string | null>(null);
  const [rename, setRename] = useState('');

  const act = (method: string, params?: unknown) => call<UiState>(method, params).then((s) => { onState(s); setError(null); }, (e: unknown) => setError(e instanceof Error ? e.message : String(e)));

  return (
    <div className="page">
      <div className="card">
        <div className="label">Connected sites on {state.network.name}</div>
        {state.connectedOrigins.length === 0 ? <p className="muted">None.</p> : (
          <ul className="list">
            {state.connectedOrigins.map((o) => (
              <li key={o}><span>{o}</span><button className="mini" onClick={() => void act('disconnectOrigin', { origin: o })}>Disconnect</button></li>
            ))}
          </ul>
        )}
      </div>
      <div className="card">
        <div className="label">Auto-lock after (minutes)</div>
        <div className="row">
          <input type="number" min={1} max={1440} value={minutes} onChange={(e) => setMinutes(e.target.value)} />
          <button onClick={() => void act('setAutoLock', { minutes: Number(minutes) })}>Save</button>
        </div>
      </div>
      <div className="card">
        <div className="label">Rename active account</div>
        <div className="row">
          <input value={rename} placeholder={state.accounts.find((a) => a.index === state.activeIndex)?.name ?? ''} onChange={(e) => setRename(e.target.value)} />
          <button onClick={() => void act('renameAccount', { index: state.activeIndex, name: rename }).then(() => setRename(''))}>Save</button>
        </div>
      </div>
      <div className="card">
        <div className="label">Seed phrase</div>
        <p className="muted small">Enter your password to reveal it. Never share it.</p>
        <div className="row">
          <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="Password" autoComplete="current-password" />
          <button onClick={() => void call<{ mnemonic: string }>('revealMnemonic', { password }).then((r) => { setMnemonic(r.mnemonic); setError(null); }, (e: unknown) => setError(String(e instanceof Error ? e.message : e)))}>Reveal</button>
        </div>
        {mnemonic && <p className="words-inline">{mnemonic}</p>}
        <button className="danger" onClick={() => { if (confirm('Erase this wallet from the browser? Only the seed phrase can restore it.')) void act('resetWallet', { password }); }}>Reset wallet</button>
      </div>
      <div className="card muted small">
        <div>Networks: {state.networks.map((n) => `${n.name}${n.rpc ? ` (${n.rpc})` : ''}`).join(' · ')}</div>
      </div>
      {error && <p className="error">{error}</p>}
    </div>
  );
}
