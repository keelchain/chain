import { OrcaMark } from './marks';
import { useEffect, useState, type ReactNode } from 'react';
import { useDecimals } from '../api/hooks';
import { formatAmount, formatExact, formatRelative, truncateHex } from '../lib/format';

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

export function Hex({ value, full = false, head, tail, copy = true }: { value: string; full?: boolean; head?: number; tail?: number; copy?: boolean }) {
  return (
    <span className={`hex${full ? ' full' : ''}`} title={full ? undefined : value}>
      <span>{full ? value : truncateHex(value, head, tail)}</span>
      {copy && <CopyButton text={value} />}
    </span>
  );
}

/** Smallest units of `asset` as a human number; the exact value is in the tooltip. */
export function Amount({ value, asset, sym = true }: { value: string | number | bigint | null | undefined; asset: string; sym?: boolean }) {
  const dec = useDecimals();
  const d = dec(asset);
  const full = formatAmount(value, d, { maxFraction: d });
  return (
    <span className="amount" title={value === null || value === undefined ? undefined : `${full} ${asset}${asset === 'KEEL' ? ` (${String(value)} orcas)` : ''}`}>
      {asset === 'KEEL' && <OrcaMark className="orca" />}
      {formatAmount(value, d)}
      {sym && <span className="sym">{asset}</span>}
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

export type Tone = 'good' | 'warn' | 'bad' | 'neutral' | 'accent';

export function Badge({ tone = 'neutral', children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`badge ${tone === 'neutral' ? '' : tone}`}>{children}</span>;
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

export function KV({ rows }: { rows: [ReactNode, ReactNode][] }) {
  return (
    <dl className="kv">
      {rows.map(([k, v], i) => (
        <div key={i} className="kv-row">
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}

export function Tabs<T extends string>({ tabs, value, onChange, label }: { tabs: { id: T; label: ReactNode }[]; value: T; onChange: (t: T) => void; label: string }) {
  return (
    <div className="tabs" role="tablist" aria-label={label}>
      {tabs.map((t) => (
        <button key={t.id} type="button" role="tab" aria-selected={t.id === value} className={t.id === value ? 'active' : ''} onClick={() => onChange(t.id)}>
          {t.label}
        </button>
      ))}
    </div>
  );
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
  const unreachable = e?.status === 0 || e?.code === 'NETWORK';
  return (
    <div className="error-box" role="alert">
      {unreachable ? (
        <>
          <strong>Network unavailable.</strong> The node for this network is not reachable, so there is no {what} to show. Try again in a moment.
        </>
      ) : (
        <>
          <strong>Could not load {what}.</strong> <code>{e?.code ?? 'ERROR'}</code> {e?.message}
        </>
      )}
    </div>
  );
}

/** A callout: `warn` for something to act on, `bad` for something broken, `info` otherwise. */
export function Notice({ tone = 'info', title, children, action }: { tone?: 'info' | 'warn' | 'bad'; title?: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className={`notice ${tone}`} role={tone === 'info' ? 'note' : 'alert'}>
      <div className="notice-text">
        {title && <strong>{title}</strong>}
        {children && <div>{children}</div>}
      </div>
      {action}
    </div>
  );
}
