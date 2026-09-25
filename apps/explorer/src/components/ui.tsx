import { useEffect, useState, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { useHref, useNetwork } from '../network/NetworkContext';
import { formatAmount, formatExact, formatPrice, formatRelative, splitPair, truncateHex } from '../lib/format';
import { externalAddressUrl, externalTxUrl } from '../lib/links';
import { useDecimals } from '../lib/assets';

// ---------------------------------------------------------------- copy

export function CopyButton({ text, label = 'Copy' }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  useEffect(() => {
    if (!done) return;
    const t = setTimeout(() => setDone(false), 1200);
    return () => clearTimeout(t);
  }, [done]);
  return (
    <button
      type="button"
      className={`copy-btn${done ? ' done' : ''}`}
      title={done ? 'Copied' : label}
      aria-label={done ? 'Copied' : `${label} ${text}`}
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        void navigator.clipboard?.writeText(text).then(() => setDone(true)).catch(() => setDone(true));
      }}
    >
      {done ? (
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" aria-hidden><path d="M5 13l4 4L19 7" /></svg>
      ) : (
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden><rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15V5a2 2 0 0 1 2-2h10" /></svg>
      )}
    </button>
  );
}

// ---------------------------------------------------------------- hex + links

export function Hex({ value, full = false, head, tail, copy = true }: { value: string; full?: boolean; head?: number; tail?: number; copy?: boolean }) {
  return (
    <span className={`hex${full ? ' full' : ''}`} title={full ? undefined : value}>
      <span>{full ? value : truncateHex(value, head, tail)}</span>
      {copy && <CopyButton text={value} />}
    </span>
  );
}

function isSystemAccount(addr: string): boolean {
  return !/^[0-9a-f]{64}$/i.test(addr);
}

export function AddressLink({ address, full = false, copy = true }: { address: string; full?: boolean; copy?: boolean }) {
  const href = useHref();
  if (!address) return <span className="muted">—</span>;
  if (isSystemAccount(address)) {
    return (
      <span className="hex" title="System or escrow account (no key; balances are ledger-only)">
        <span className="badge">{address}</span>
      </span>
    );
  }
  return (
    <span className={`hex${full ? ' full' : ''}`} title={address}>
      <Link to={href(`/account/${address}`)}>{full ? address : truncateHex(address)}</Link>
      {copy && <CopyButton text={address} />}
    </span>
  );
}

export function TxLink({ id, full = false }: { id: string; full?: boolean }) {
  const href = useHref();
  return (
    <span className={`hex${full ? ' full' : ''}`} title={id}>
      <Link to={href(`/tx/${id}`)}>{full ? id : truncateHex(id, 10, 6)}</Link>
      <CopyButton text={id} />
    </span>
  );
}

export function BlockLink({ height }: { height: number }) {
  const href = useHref();
  return (
    <Link to={href(`/blocks/${height}`)} className="mono">
      {height.toLocaleString('en-US')}
    </Link>
  );
}

export function PairLink({ pair }: { pair: string }) {
  const href = useHref();
  return <Link to={href(`/markets/${encodeURIComponent(pair)}`)}>{pair}</Link>;
}

export function AssetLink({ asset }: { asset: string }) {
  const href = useHref();
  return <Link to={href(`/assets/${encodeURIComponent(asset)}`)}>{asset}</Link>;
}

export function ExternalLink({ chain, value, what }: { chain: string; value: string; what: 'tx' | 'address' }) {
  const { network } = useNetwork();
  const url = what === 'tx' ? externalTxUrl(chain, value, network.id) : externalAddressUrl(chain, value, network.id);
  return (
    <span className="hex" title={value}>
      {url ? (
        <a href={url} target="_blank" rel="noreferrer noopener">
          {truncateHex(value, 10, 8)} ↗
        </a>
      ) : (
        <span>{truncateHex(value, 10, 8)}</span>
      )}
      <CopyButton text={value} />
    </span>
  );
}

// ---------------------------------------------------------------- amounts + time

export function Amount({ value, asset, decimals, compact = false, sym = true }: { value: string | number | null | undefined; asset?: string; decimals?: number; compact?: boolean; sym?: boolean }) {
  const dec = useDecimals();
  const d = decimals ?? (asset ? dec(asset) : 6);
  const text = formatAmount(value, d, { compact });
  const full = formatAmount(value, d, { maxFraction: d });
  return (
    <span className="amount" title={value === null || value === undefined ? undefined : `${full}${asset ? ` ${asset}` : ''}`}>
      {text}
      {sym && asset && <span className="sym">{asset}</span>}
    </span>
  );
}

export function Price({ value, pair }: { value: string | null | undefined; pair: string }) {
  const dec = useDecimals();
  const { quote } = splitPair(pair);
  return (
    <span className="amount" title={value ? `${formatAmount(value, dec(quote), { maxFraction: dec(quote) })} ${quote}` : undefined}>
      {formatPrice(value, dec(quote))}
      <span className="sym">{quote}</span>
    </span>
  );
}

export function TimeAgo({ ts }: { ts: number | null | undefined }) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 10_000);
    return () => clearInterval(t);
  }, []);
  if (!ts) return <span className="muted">—</span>;
  return (
    <time dateTime={new Date(ts).toISOString()} title={formatExact(ts)}>
      {formatRelative(ts)}
    </time>
  );
}

// ---------------------------------------------------------------- chrome

export function Badge({ tone = 'neutral', children }: { tone?: 'good' | 'warn' | 'bad' | 'neutral' | 'accent'; children: ReactNode }) {
  return <span className={`badge ${tone === 'neutral' ? '' : tone}`}>{children}</span>;
}

export function StatusBadge({ status }: { status: string }) {
  const s = status.toLowerCase();
  const tone: 'good' | 'warn' | 'bad' | 'neutral' | 'accent' =
    ['ok', 'active', 'released', 'credited', 'confirmed', 'executed', 'passed', 'filled', 'open'].includes(s)
      ? 'good'
      : ['paused', 'paid', 'held', 'pending', 'voting', 'batched', 'queued', 'signing', 'broadcast', 'partially_filled', 'funded', 'disputed'].includes(s)
        ? 'warn'
        : ['closed', 'cancelled', 'rejected', 'failed', 'vetoed', 'jailed', 'error'].includes(s)
          ? 'bad'
          : s === 'ruled'
            ? 'accent'
            : 'neutral';
  return <Badge tone={tone}>{status.replace(/_/g, ' ')}</Badge>;
}

export function OkBadge({ ok, code }: { ok: boolean; code?: string }) {
  return ok ? <Badge tone="good">OK</Badge> : <Badge tone="bad">{code ?? 'Error'}</Badge>;
}

export function Card({ title, actions, children, flush = false }: { title?: ReactNode; actions?: ReactNode; children: ReactNode; flush?: boolean }) {
  return (
    <section className="card">
      {(title || actions) && (
        <div className="card-head">
          {typeof title === 'string' ? <h2>{title}</h2> : title}
          <span className="spacer" />
          {actions}
        </div>
      )}
      <div className={`card-body${flush ? ' flush' : ''}`}>{children}</div>
    </section>
  );
}

export function StatTile({ label, value, hint }: { label: string; value: ReactNode; hint?: ReactNode }) {
  return (
    <div className="tile">
      <div className="label">{label}</div>
      <div className="value">{value}</div>
      {hint && <div className="hint">{hint}</div>}
    </div>
  );
}

export function KV({ rows }: { rows: [ReactNode, ReactNode][] }) {
  return (
    <dl className="kv">
      {rows.map(([k, v], i) => (
        <RowPair key={i} k={k} v={v} />
      ))}
    </dl>
  );
}

function RowPair({ k, v }: { k: ReactNode; v: ReactNode }) {
  return (
    <>
      <dt>{k}</dt>
      <dd>{v}</dd>
    </>
  );
}

export function Tabs<T extends string>({ tabs, value, onChange }: { tabs: { id: T; label: ReactNode }[]; value: T; onChange: (t: T) => void }) {
  return (
    <div className="tabs" role="tablist">
      {tabs.map((t) => (
        <button key={t.id} type="button" role="tab" aria-selected={t.id === value} className={t.id === value ? 'active' : ''} onClick={() => onChange(t.id)}>
          {t.label}
        </button>
      ))}
    </div>
  );
}

export function Empty({ children = 'Nothing here yet.' }: { children?: ReactNode }) {
  return <div className="empty">{children}</div>;
}

export function Loading({ rows = 3 }: { rows?: number }) {
  return (
    <div className="card-body" aria-busy="true" aria-label="Loading">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} style={{ padding: '6px 0' }}>
          <span className="skeleton" style={{ width: `${40 + ((i * 23) % 50)}%` }} />
        </div>
      ))}
    </div>
  );
}

export function ErrorState({ error, what = 'data' }: { error: unknown; what?: string }) {
  const e = error as { status?: number; code?: string; message?: string } | undefined;
  const notFound = e?.status === 404;
  const unreachable = e?.status === 0 || e?.code === 'NETWORK';
  return (
    <div className="error-box" role="alert">
      {notFound ? (
        <>
          <strong>Not found.</strong> The indexer has no {what} matching this reference.
        </>
      ) : unreachable ? (
        <>
          <strong>Network unavailable.</strong> The indexer for this network is not reachable. Try the other network from the switcher or retry later.
        </>
      ) : (
        <>
          <strong>Could not load {what}.</strong> <code>{e?.code ?? 'ERROR'}</code> {e?.message}
        </>
      )}
    </div>
  );
}

export function JsonView({ value }: { value: unknown }) {
  const text = JSON.stringify(value, (_k, v) => (typeof v === 'bigint' ? v.toString() : v), 2);
  return (
    <div style={{ position: 'relative' }}>
      <pre className="json">{text}</pre>
      <span style={{ position: 'absolute', top: 8, right: 8 }}>
        <CopyButton text={text} label="Copy JSON" />
      </span>
    </div>
  );
}

export function Pager({ hasMore, onMore, loading }: { hasMore: boolean; onMore: () => void; loading?: boolean }) {
  if (!hasMore) return null;
  return (
    <div style={{ padding: 12, textAlign: 'center' }}>
      <button type="button" className="btn" onClick={onMore} disabled={loading}>
        {loading ? 'Loading…' : 'Load more'}
      </button>
    </div>
  );
}

export function Meter({ ratio, tone }: { ratio: number; tone?: 'warn' | 'bad' }) {
  const pct = Math.max(0, Math.min(1, ratio)) * 100;
  return (
    <div className={`meter${tone ? ` ${tone}` : ''}`} role="meter" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100}>
      <div style={{ width: `${pct}%` }} />
    </div>
  );
}
