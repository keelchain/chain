/**
 * One ratio against its limit: a name, a thin bar and the figures. The
 * fill carries the state and the track is a lighter step of the same hue,
 * so the state reads across the whole bar; the state is also written out,
 * never colour alone.
 */
import type { ReactNode } from 'react';

export type MeterTone = 'ok' | 'warn' | 'bad';

export function MeterRow({
  label,
  ratio,
  figure,
  note,
  tone = 'ok',
  state,
}: {
  label: string;
  /** 0..1 of the bar that is filled. */
  ratio: number;
  /** The numbers behind the bar, e.g. "742 of 1,200 USDT left". */
  figure: ReactNode;
  note?: ReactNode;
  tone?: MeterTone;
  /** The state in a word or two, shown when it is not `ok`. */
  state?: string;
}) {
  const pct = Math.round(Math.max(0, Math.min(1, ratio)) * 100);
  return (
    <div className="meter-row">
      <div className="meter-head">
        <span className="meter-label">{label}</span>
        <span className="meter-figure">
          {tone !== 'ok' && state && (
            <span className={`meter-state ${tone}`}>
              <span aria-hidden>{tone === 'bad' ? '▲' : '●'}</span> {state}
            </span>
          )}
          {figure}
        </span>
      </div>
      <div className={`meter ${tone === 'ok' ? '' : tone}`} role="meter" aria-label={label} aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-valuetext={`${pct}%${state ? `, ${state}` : ''}`}>
        <div style={{ width: `${pct}%` }} />
      </div>
      {note && <div className="meter-note">{note}</div>}
    </div>
  );
}
