import { useEffect, useState } from 'react';
import { call } from '../ui';

export interface CurrentSite {
  origin: string;
  tabId: number | null;
}

/** The site of the tab the popup was opened on (activeTab), or null for a browser page. */
export async function currentSite(): Promise<CurrentSite | null> {
  try {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tab?.url) return null;
    const u = new URL(tab.url);
    if (!['https:', 'http:'].includes(u.protocol)) return null;
    return { origin: u.origin, tabId: typeof tab.id === 'number' ? tab.id : null };
  } catch {
    return null;
  }
}

/** The sites the manifest itself covers: Keelchain's own and local development. */
export function isBuiltInSite(origin: string): boolean {
  try {
    const { protocol, hostname } = new URL(origin);
    if (protocol === 'https:' && (hostname === 'keelchain.com' || hostname.endsWith('.keelchain.com'))) return true;
    return protocol === 'http:' && (hostname === 'localhost' || hostname === '127.0.0.1');
  } catch {
    return false;
  }
}

/**
 * Asks the browser for the origin (the prompt has to come from a click in
 * the popup), registers the wallet for it and puts the provider on the
 * open page. Returns the enabled sites and whether the open page got it.
 */
export async function enableSiteAccess(origin: string, tabId: number | null): Promise<{ sites: string[]; injected: boolean }> {
  const granted = await chrome.permissions.request({ origins: [`${origin}/*`] });
  if (!granted) throw new Error('Access to that site was not granted.');
  const r = await call<{ sites: string[]; injected?: boolean }>('enableSite', tabId === null ? { origin } : { origin, tabId });
  return { sites: r.sites, injected: r.injected === true };
}

/**
 * The first thing somebody sees when they open the wallet on a site it is
 * not enabled for: one button, instead of a setting they have to know
 * about. Nothing renders on Keelchain's own sites, on enabled sites or on
 * browser pages.
 */
export function SiteAccessBanner() {
  const [site, setSite] = useState<CurrentSite | null>(null);
  const [enabled, setEnabled] = useState<string[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<'ready' | 'reload' | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void currentSite().then(setSite);
    void call<{ sites: string[] }>('listSites').then((r) => setEnabled(r.sites), () => setEnabled([]));
  }, []);

  if (site === null || enabled === null || isBuiltInSite(site.origin)) return null;
  const host = new URL(site.origin).host;
  if (done !== null) {
    return (
      <div className="card" data-site-access="enabled">
        <p className="small">
          Keel Wallet is enabled on <strong>{host}</strong>.{' '}
          {done === 'ready' ? 'Go back to the page and continue.' : 'Reload the page to use it.'}
        </p>
      </div>
    );
  }
  if (enabled.includes(site.origin)) return null;

  const enable = async () => {
    setBusy(true);
    try {
      const r = await enableSiteAccess(site.origin, site.tabId);
      setEnabled(r.sites);
      setDone(r.injected ? 'ready' : 'reload');
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card notice" data-site-access="needed">
      <p className="small">
        <strong>{host}</strong> cannot see Keel Wallet yet. Enable it to sign in or sign up there with this wallet. The
        browser asks for permission once.
      </p>
      <button disabled={busy} onClick={() => void enable()}>Enable on this site</button>
      {error !== null && <p className="error small">{error}</p>}
    </div>
  );
}
