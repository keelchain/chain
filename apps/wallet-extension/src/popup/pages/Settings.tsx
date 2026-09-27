import { useEffect, useState } from 'react';
import { call, type UiState } from '../ui';
import { NETWORKS } from '../../core/networks';

/** The origin of the tab the popup was opened on (activeTab), or null. */
async function currentOrigin(): Promise<string | null> {
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab?.url) return null;
    const u = new URL(tab.url);
    if (!['https:', 'http:'].includes(u.protocol)) return null;
    return u.origin;
  } catch {
    return null;
  }
}

export function Settings({ state, onState }: { state: UiState; onState: (s: UiState) => void }) {
  const [error, setError] = useState<string | null>(null);
  const [sites, setSites] = useState<string[]>([]);
  const [here, setHere] = useState<string | null>(null);
  const [siteInput, setSiteInput] = useState('');
  const [netName, setNetName] = useState('');
  const [netRpc, setNetRpc] = useState('');
  const [netExplorer, setNetExplorer] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void call<{ sites: string[] }>('listSites').then((r) => setSites(r.sites), () => undefined);
    void currentOrigin().then(setHere);
  }, []);

  const enableSite = async (origin: string) => {
    setBusy(true);
    try {
      // The permission prompt must come from this click, in the popup.
      const granted = await chrome.permissions.request({ origins: [`${origin}/*`] });
      if (!granted) throw new Error('Access to that site was not granted.');
      const r = await call<{ sites: string[] }>('enableSite', { origin });
      setSites(r.sites);
      setSiteInput('');
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const disableSite = (origin: string) => call<{ sites: string[] }>('disableSite', { origin }).then((r) => { setSites(r.sites); setError(null); }, (e: unknown) => setError(e instanceof Error ? e.message : String(e)));
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
      <div className="card">
        <div className="label">Sites that can use the wallet</div>
        <p className="muted small">Keelchain's own sites and localhost always can. Enable any other site here; the browser asks for permission once.</p>
        {here && !sites.includes(here) && (
          <div className="row">
            <span className="small">{here}</span>
            <button disabled={busy} onClick={() => void enableSite(here)}>Enable on this site</button>
          </div>
        )}
        <div className="row">
          <input value={siteInput} placeholder="https://exchange.example" onChange={(e) => setSiteInput(e.target.value)} />
          <button disabled={busy || siteInput.trim().length === 0} onClick={() => { try { void enableSite(new URL(siteInput.trim()).origin); } catch { setError('Enter a site URL.'); } }}>Enable</button>
        </div>
        {sites.length > 0 && (
          <ul className="list">
            {sites.map((o) => (
              <li key={o}><span>{o}</span><button className="mini" onClick={() => void disableSite(o)}>Remove</button></li>
            ))}
          </ul>
        )}
      </div>
      <div className="card">
        <div className="label">Networks</div>
        <ul className="list">
          {state.networks.map((n) => (
            <li key={n.id}>
              <span>{n.name} <span className="muted small">chain {n.chainId}{n.rpc ? ` · ${n.rpc}` : ''}</span></span>
              {!NETWORKS.some((b) => b.id === n.id) && <button className="mini" onClick={() => void act('removeNetwork', { id: n.id })}>Remove</button>}
            </li>
          ))}
        </ul>
        <p className="muted small">Add a network you run or were given; its chain id is read from the RPC.</p>
        <div className="row"><input value={netName} placeholder="Name" onChange={(e) => setNetName(e.target.value)} /></div>
        <div className="row"><input value={netRpc} placeholder="https://host/rpc" onChange={(e) => setNetRpc(e.target.value)} /></div>
        <div className="row">
          <input value={netExplorer} placeholder="Explorer URL (optional)" onChange={(e) => setNetExplorer(e.target.value)} />
          <button disabled={busy} onClick={() => { setBusy(true); void act('addNetwork', { name: netName, rpc: netRpc, explorer: netExplorer }).then(() => { setNetName(''); setNetRpc(''); setNetExplorer(''); }).finally(() => setBusy(false)); }}>Add</button>
        </div>
      </div>
      {error && <p className="error">{error}</p>}
    </div>
  );
}
