import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useClients } from '../api/hooks';
import type { ClientListing } from '../api/types';
import { AddressLink, Badge, Card, Empty, ErrorState, Loading, StatTile } from '../components/ui';
import { formatInt } from '../lib/format';
import { useHref } from '../network/NetworkContext';

/** The exchange's logo once its website is proved, otherwise its initial: an unproved link is not loaded. */
function Mark({ c }: { c: ClientListing }) {
  const [broken, setBroken] = useState(false);
  const name = c.profile?.name ?? '';
  if (c.website_verified === true && c.profile?.logo && !broken) {
    return <img className="exchange-mark" src={c.profile.logo} alt="" width={28} height={28} loading="lazy" referrerPolicy="no-referrer" onError={() => setBroken(true)} />;
  }
  return (
    <span className="exchange-mark initial" aria-hidden="true">
      {(name[0] ?? '?').toUpperCase()}
    </span>
  );
}

function Website({ c }: { c: ClientListing }) {
  const site = c.profile?.website;
  if (!site) return <span className="muted">—</span>;
  return (
    <span className="exchange-name">
      <a href={`https://${site}`} target="_blank" rel="noreferrer noopener nofollow">
        {site}
      </a>
      {c.website_verified === true ? <Badge tone="good">Verified</Badge> : c.website_verified === false ? <Badge tone="warn">Not verified</Badge> : <Badge>Checking</Badge>}
    </span>
  );
}

export function ExchangesPage() {
  const q = useClients();
  const href = useHref();
  if (q.isLoading) return <Loading rows={8} />;
  if (q.isError) return <ErrorState error={q.error} what="exchanges" />;
  const list = q.data ?? [];
  const named = list.filter((c) => c.profile);
  return (
    <div className="stack">
      <div className="page-head">
        <h1>Exchanges</h1>
        <span className="sub">Registered clients that run on Keel. A name is unique on chain; a website is verified when it serves the exchange's address</span>
      </div>
      <div className="tiles">
        <StatTile label="Registered" value={formatInt(list.length)} hint={`${named.length} with a public profile`} />
        <StatTile label="Verified websites" value={formatInt(list.filter((c) => c.website_verified === true).length)} />
        <StatTile label="Open offers" value={formatInt(list.reduce((n, c) => n + c.open_offers, 0))} hint="listed under an exchange" />
        <StatTile label="Accounts" value={formatInt(list.reduce((n, c) => n + c.attested_accounts, 0))} hint="vouched for by an exchange" />
      </div>
      <Card flush>
        {list.length === 0 ? (
          <Empty>No exchange is registered yet.</Empty>
        ) : (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>Exchange</th>
                  <th>Website</th>
                  <th>Account</th>
                  <th className="num">Accounts</th>
                  <th className="num">Open offers</th>
                  <th>Plan</th>
                </tr>
              </thead>
              <tbody>
                {list.map((c) => (
                  <tr key={c.address}>
                    <td>
                      <span className="exchange-name">
                        <Mark c={c} />
                        {c.profile ? <strong>{c.profile.name}</strong> : <span className="muted">No public profile</span>}
                      </span>
                    </td>
                    <td>
                      <Website c={c} />
                    </td>
                    <td>
                      <AddressLink address={c.address} />
                    </td>
                    <td className="num">{formatInt(c.attested_accounts)}</td>
                    <td className="num">{c.open_offers > 0 ? <Link to={href(`/offers?client=${c.address}&status=active`)}>{formatInt(c.open_offers)}</Link> : '0'}</td>
                    <td>{c.plan_active ? <Badge tone="good">Active</Badge> : <Badge>None</Badge>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </div>
  );
}
