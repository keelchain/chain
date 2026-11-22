import { HarborMark } from './marks';
import { Component, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { useHref, useNetwork } from '../network/NetworkContext';
import { explorerUrl, isMockEnabled } from '../network/networks';
import { useWallet } from '../wallet/WalletContext';
import { Hex } from './ui';

const NAV: { to: string; label: string; end?: boolean }[] = [
  { to: '/', label: 'Overview', end: true },
  { to: '/plans', label: 'Plans' },
  { to: '/funds', label: 'Funds' },
  { to: '/usage', label: 'Usage' },
  { to: '/users', label: 'Users' },
  { to: '/business', label: 'Business' },
];

export const DOCS_URL = 'https://keelchain.com/docs/exchange/';
export const WALLET_URL = 'https://keelchain.com/wallet/';

function NetworkSwitcher() {
  const { network, networks } = useNetwork();
  const navigate = useNavigate();
  const location = useLocation();
  if (networks.length < 2) return <span className="badge">{network.name}</span>;
  return (
    <select
      className="select"
      aria-label="Network"
      value={network.id}
      onChange={(e) => {
        // Keep the rest of the path so the same page opens on the other network.
        const rest = location.pathname.replace(new RegExp(`^/${network.id}`), '');
        navigate(`/${e.target.value}${rest}`);
      }}
    >
      {networks.map((n) => (
        <option key={n.id} value={n.id}>
          {n.name}
        </option>
      ))}
    </select>
  );
}

/** Header control: Connect, or the connected address with a small menu. */
function ConnectControl() {
  const w = useWallet();
  const { network } = useNetwork();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', close);
    document.addEventListener('keydown', esc);
    return () => {
      document.removeEventListener('mousedown', close);
      document.removeEventListener('keydown', esc);
    };
  }, [open]);
  if (!w.account) {
    return (
      <button type="button" className="btn primary" onClick={() => void w.connect()} disabled={w.busy} title={w.error ?? 'Connect Keel Wallet'}>
        {w.busy ? 'Connecting…' : 'Connect'}
      </button>
    );
  }
  return (
    <div className="wallet-menu" ref={ref}>
      <button type="button" className="btn" onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open}>
        <span className="network-dot" aria-hidden /> <Hex value={w.account.address} head={6} tail={4} copy={false} />
      </button>
      {open && (
        <div className="wallet-dropdown" role="menu">
          <a className="btn" role="menuitem" href={explorerUrl(network.id, `/account/${w.account.address}`)} target="_blank" rel="noreferrer noopener">
            Account in the explorer ↗
          </a>
          <button type="button" className="btn" role="menuitem" onClick={() => void w.disconnect().then(() => setOpen(false))}>
            Disconnect
          </button>
        </div>
      )}
    </div>
  );
}

function Header() {
  const href = useHref();
  const { network } = useNetwork();
  const w = useWallet();
  return (
    <header className="header">
      <div className="header-inner">
        <Link to={href('/')} className="brand">
          <img className="brand-mark" src={`${import.meta.env.BASE_URL}logo-mark.svg`} alt="" width={27} height={24} />
          Keel Harbor
          <HarborMark className="brand-harbor" size={20} />
        </Link>
        {w.account && (
          <nav className="nav" aria-label="Primary">
            {NAV.map((n) => (
              <NavLink key={n.to} to={href(n.to)} end={n.end} className={({ isActive }) => (isActive ? 'active' : '')}>
                {n.label}
              </NavLink>
            ))}
          </nav>
        )}
        <div className="header-right">
          <a className="header-link" href={explorerUrl(network.id)} target="_blank" rel="noreferrer noopener">
            Explorer ↗
          </a>
          <NetworkSwitcher />
          <ConnectControl />
        </div>
      </div>
    </header>
  );
}

function Footer() {
  const { network } = useNetwork();
  return (
    <footer className="footer">
      <div className="footer-inner">
        <span>Keel Harbor</span>
        <span>
          {network.name}
          {isMockEnabled() ? ' · demo data' : ''}
        </span>
        <a href={DOCS_URL}>Client docs</a>
        <span className="footer-note">No server and no account of its own: every number is read from the chain and every change is an action you sign in Keel Wallet.</span>
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
              </button>
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
      <HarborMark className="watermark" size={520} />
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
