import { Component, useEffect, useState, type FormEvent, type ReactNode } from 'react';
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { useHealth } from '../api/hooks';
import { resolveSearch } from '../lib/search';
import { applyTheme, nextTheme, readTheme, type ThemeChoice } from '../lib/theme';
import { useHref, useNetwork } from '../network/NetworkContext';
import { isMockEnabled } from '../network/networks';

const NAV: { to: string; label: string }[] = [
  { to: '/blocks', label: 'Blocks' },
  { to: '/txs', label: 'Transactions' },
  { to: '/assets', label: 'Assets' },
  { to: '/markets', label: 'Markets' },
  { to: '/offers', label: 'Offers' },
  { to: '/validators', label: 'Validators' },
  { to: '/vaults', label: 'Vaults' },
  { to: '/governance', label: 'Governance' },
];

export function NetworkSwitcher() {
  const { network, networks } = useNetwork();
  const navigate = useNavigate();
  const location = useLocation();
  const health = useHealth();
  const status = health.isError ? 'off' : health.data && health.data.lag > 20 ? 'lag' : health.data ? 'ok' : 'off';
  const title = health.isError
    ? 'Indexer unreachable'
    : health.data
      ? `${health.data.chain_id} — indexed ${health.data.indexed_height.toLocaleString('en-US')} / node ${health.data.node_height.toLocaleString('en-US')}${health.data.state_hash_ok ? '' : ' — STATE HASH MISMATCH'}`
      : 'Connecting…';
  return (
    <label className="network-switch" title={title}>
      <span className={`network-dot ${status === 'ok' ? '' : status}`} aria-hidden />
      <select
        className="select"
        aria-label="Network"
        value={network.id}
        onChange={(e) => {
          const id = e.target.value;
          // Keep the rest of the path so the same page opens on the other network.
          const rest = location.pathname.replace(new RegExp(`^/${network.id}`), '');
          navigate(`/${id}${rest}${location.search}`);
        }}
      >
        {networks.map((n) => (
          <option key={n.id} value={n.id}>
            {n.name}
          </option>
        ))}
      </select>
    </label>
  );
}

export function SearchBox({ autoFocus = false }: { autoFocus?: boolean }) {
  const { api } = useNetwork();
  const href = useHref();
  const navigate = useNavigate();
  const [q, setQ] = useState('');
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!q.trim() || busy) return;
    setBusy(true);
    try {
      const r = await resolveSearch(api, q);
      if (r && 'path' in r) {
        navigate(href(r.path));
        setQ('');
      } else {
        navigate(`${href('/search')}?q=${encodeURIComponent(q.trim())}`);
      }
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="search-form" role="search" onSubmit={(e) => void submit(e)}>
      <input aria-label="Search" placeholder="Search block, tx, address, asset, pair, offer #…" value={q} onChange={(e) => setQ(e.target.value)} autoFocus={autoFocus} />
      <button type="submit" aria-label="Go" disabled={busy}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></svg>
      </button>
    </form>
  );
}

function ThemeToggle() {
  const [choice, setChoice] = useState<ThemeChoice>(() => (typeof window === 'undefined' ? 'system' : readTheme()));
  useEffect(() => {
    applyTheme(choice);
  }, [choice]);
  const label = choice === 'system' ? 'Theme: system' : choice === 'dark' ? 'Theme: dark' : 'Theme: light';
  return (
    <button type="button" className="btn" onClick={() => setChoice(nextTheme(choice))} title={label} aria-label={label}>
      {choice === 'system' ? 'Auto' : choice === 'dark' ? 'Dark' : 'Light'}
    </button>
  );
}

export function Header() {
  const href = useHref();
  return (
    <header className="header">
      <div className="header-inner">
        <Link to={href('/')} className="brand">
          <img className="brand-mark" src={`${import.meta.env.BASE_URL}logo-mark.svg`} alt="" width={27} height={24} />
          Keel Explorer
        </Link>
        <nav className="nav" aria-label="Primary">
          {NAV.map((n) => (
            <NavLink key={n.to} to={href(n.to)} className={({ isActive }) => (isActive ? 'active' : '')}>
              {n.label}
            </NavLink>
          ))}
        </nav>
        <div className="header-right">
          <SearchBox />
          <NetworkSwitcher />
          <ThemeToggle />
        </div>
      </div>
    </header>
  );
}

export function Footer() {
  const { network } = useNetwork();
  const health = useHealth();
  return (
    <footer className="footer">
      <div className="footer-inner">
        <span>Keelchain explorer</span>
        <span>
          {network.name} · {isMockEnabled() ? 'mock data' : network.api}
        </span>
        {health.data && (
          <span>
            indexed {health.data.indexed_height.toLocaleString('en-US')} · lag {health.data.lag} · state hash {health.data.state_hash_ok ? 'verified' : 'MISMATCH'}
          </span>
        )}
        <span style={{ marginLeft: 'auto' }}>No gas: actions are free under a per-address budget; fees only on fills, releases and withdrawals.</span>
      </div>
    </footer>
  );
}

export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  override state: { error: Error | null } = { error: null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  override render() {
    if (this.state.error) {
      return (
        <div className="main">
          <div className="error-box" role="alert">
            <strong>Something went wrong rendering this page.</strong>
            <div className="mono" style={{ marginTop: 6 }}>
              {this.state.error.message}
            </div>
            <div style={{ marginTop: 10 }}>
              <button type="button" className="btn" onClick={() => this.setState({ error: null })}>
                Try again
              </button>{' '}
              <a href="/" className="btn">
                Home
              </a>
            </div>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

export function Layout() {
  const location = useLocation();
  return (
    <div className="app">
      <Header />
      <main className="main">
        <ErrorBoundary key={location.pathname}>
          <Outlet />
        </ErrorBoundary>
      </main>
      <Footer />
    </div>
  );
}
