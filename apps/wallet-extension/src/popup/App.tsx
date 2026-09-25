import { useCallback, useEffect, useState } from 'react';
import { call, getState, type UiState } from './ui';
import { Onboarding } from './pages/Onboarding';
import { Unlock } from './pages/Unlock';
import { Home } from './pages/Home';
import { Settings } from './pages/Settings';
import { Approval } from './pages/Approval';

export type Tab = 'home' | 'settings';

export function App() {
  const [state, setState] = useState<UiState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('home');
  const approveMode = window.location.hash === '#approve';

  const refresh = useCallback(async () => {
    try {
      setState(await getState());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Poll for new approvals / lock changes and keep the service worker alive while open.
    const poll = setInterval(() => void refresh(), 1500);
    const keepalive = setInterval(() => void call('ping').catch(() => undefined), 20_000);
    return () => {
      clearInterval(poll);
      clearInterval(keepalive);
    };
  }, [refresh]);

  if (error) return <div className="screen"><p className="error">{error}</p></div>;
  if (!state) return <div className="screen"><p className="muted">Loading…</p></div>;

  if (!state.hasVault) return <Onboarding onDone={setState} />;
  if (state.locked) return <Unlock state={state} onUnlocked={setState} />;
  if (state.pending.length > 0) {
    const req = state.pending[0]!;
    return <Approval key={req.id} request={req} queued={state.pending.length - 1} onDecided={setState} />;
  }
  if (approveMode) {
    return (
      <div className="screen center">
        <p className="muted">Nothing left to approve.</p>
        <button onClick={() => window.close()}>Close</button>
      </div>
    );
  }
  return (
    <div className="shell">
      <div className="brand">
        <img src="./logo-mark.svg" alt="" width={27} height={24} />
        <span>Keel Wallet</span>
      </div>
      <nav className="tabs">
        <button className={tab === 'home' ? 'active' : ''} onClick={() => setTab('home')}>Wallet</button>
        <button className={tab === 'settings' ? 'active' : ''} onClick={() => setTab('settings')}>Settings</button>
        <button className="ghost" onClick={() => void call<UiState>('lock').then(setState)} title="Lock now">Lock</button>
      </nav>
      {tab === 'home' ? <Home state={state} onState={setState} /> : <Settings state={state} onState={setState} />}
    </div>
  );
}
