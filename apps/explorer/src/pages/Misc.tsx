import { useEffect, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { SearchBox } from '../components/Layout';
import { Card, Empty, Loading } from '../components/ui';
import { pathFor, resolveSearch, type SearchTarget } from '../lib/search';
import { useHref, useNetwork } from '../network/NetworkContext';
import { NETWORKS } from '../network/networks';

export function NotFoundPage() {
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Page not found</h1>
      </div>
      <Card>
        <p>Nothing lives at this address. Try a search, or go back to the overview.</p>
        <SearchBox />
      </Card>
    </div>
  );
}

export function UnknownNetworkPage({ id }: { id: string }) {
  return (
    <div className="app">
      <main className="main">
        <div className="stack">
          <div className="page-head">
            <h1>Unknown network “{id}”</h1>
          </div>
          <Card>
            <p>This explorer is configured for:</p>
            <ul>
              {NETWORKS.map((n) => (
                <li key={n.id}>
                  <Link to={`/${n.id}`}>{n.name}</Link> <span className="muted small">({n.api})</span>
                </li>
              ))}
            </ul>
          </Card>
        </div>
      </main>
    </div>
  );
}

export function SearchPage() {
  const [sp] = useSearchParams();
  const q = sp.get('q') ?? '';
  const { api } = useNetwork();
  const href = useHref();
  const [state, setState] = useState<{ loading: boolean; suggestions: SearchTarget[] }>({ loading: true, suggestions: [] });
  useEffect(() => {
    let cancelled = false;
    setState({ loading: true, suggestions: [] });
    void resolveSearch(api, q).then((r) => {
      if (cancelled) return;
      setState({ loading: false, suggestions: r && 'suggestions' in r ? r.suggestions : [] });
    });
    return () => {
      cancelled = true;
    };
  }, [api, q]);
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Search</h1>
        <span className="sub">“{q}”</span>
      </div>
      <Card>
        <SearchBox autoFocus />
      </Card>
      <Card title="Results" flush>
        {state.loading ? (
          <Loading rows={3} />
        ) : state.suggestions.length === 0 ? (
          <Empty>No match. Search accepts a block height, a 64-hex transaction id or address, an asset (BTC.BTC), a pair (BTC.BTC/KUSD), or “offer 12” / “trade 12”.</Empty>
        ) : (
          <ul style={{ margin: 0, padding: '8px 16px', listStyle: 'none' }}>
            {state.suggestions.map((s) => (
              <li key={`${s.kind}-${s.ref}`} style={{ padding: '6px 0' }}>
                <span className="badge">{s.kind}</span> <Link to={href(pathFor(s.kind, s.ref))}>{s.ref}</Link>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
