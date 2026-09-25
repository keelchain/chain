import { useEffect, useMemo, useRef, useState } from 'react';
import { CandlestickSeries, ColorType, HistogramSeries, createChart, type IChartApi, type UTCTimestamp } from 'lightweight-charts';
import type { Candle } from '../../api/types';
import { formatAmount, formatExact, formatPrice } from '../../lib/format';

interface Props {
  candles: Candle[];
  baseDecimals: number;
  quoteDecimals: number;
  base: string;
  quote: string;
  height?: number;
}

function cssVar(el: HTMLElement, name: string): string {
  return getComputedStyle(el).getPropertyValue(name).trim();
}

/** True when a 2D canvas is actually available (false under jsdom). */
function canvasAvailable(): boolean {
  try {
    const c = document.createElement('canvas');
    return typeof c.getContext === 'function' && !!c.getContext('2d');
  } catch {
    return false;
  }
}

/**
 * OHLC candles + volume histogram (lightweight-charts), colored with the
 * up/down hues from the palette and re-themed when the document theme flips.
 * Falls back to a table when no canvas is available (tests, print).
 */
export function CandleChart({ candles, baseDecimals, quoteDecimals, base, quote, height = 320 }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [themeTick, setThemeTick] = useState(0);
  const [showTable, setShowTable] = useState(false);
  const canvas = useMemo(() => canvasAvailable(), []);

  const data = useMemo(
    () =>
      candles
        .map((c) => ({
          time: c.t as UTCTimestamp,
          open: Number(BigInt(c.o)) / 10 ** quoteDecimals,
          high: Number(BigInt(c.h)) / 10 ** quoteDecimals,
          low: Number(BigInt(c.l)) / 10 ** quoteDecimals,
          close: Number(BigInt(c.c)) / 10 ** quoteDecimals,
          volume: Number(BigInt(c.v)) / 10 ** baseDecimals,
        }))
        .sort((a, b) => a.time - b.time),
    [candles, baseDecimals, quoteDecimals],
  );

  // Re-render on theme toggle (data-theme attribute) or OS preference change.
  useEffect(() => {
    const mo = new MutationObserver(() => setThemeTick((n) => n + 1));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    const mq = window.matchMedia?.('(prefers-color-scheme: dark)');
    const onMq = () => setThemeTick((n) => n + 1);
    mq?.addEventListener?.('change', onMq);
    return () => {
      mo.disconnect();
      mq?.removeEventListener?.('change', onMq);
    };
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el || !canvas || showTable) return;
    const up = cssVar(el, '--up') || '#1baf7a';
    const down = cssVar(el, '--down') || '#e34948';
    const text = cssVar(el, '--text-2') || '#52514e';
    const grid = cssVar(el, '--grid') || '#e1e0d9';
    const border = cssVar(el, '--border-strong') || '#c3c2b7';
    let chart: IChartApi;
    try {
      chart = createChart(el, {
        height,
        layout: { background: { type: ColorType.Solid, color: 'transparent' }, textColor: text, fontFamily: 'system-ui, sans-serif', fontSize: 11 },
        grid: { vertLines: { color: grid }, horzLines: { color: grid } },
        rightPriceScale: { borderColor: border },
        timeScale: { borderColor: border, timeVisible: true, secondsVisible: false },
        crosshair: { mode: 0 },
        handleScroll: true,
        handleScale: true,
        autoSize: true,
      });
    } catch {
      return;
    }
    const series = chart.addSeries(CandlestickSeries, {
      upColor: up,
      downColor: down,
      borderUpColor: up,
      borderDownColor: down,
      wickUpColor: up,
      wickDownColor: down,
      priceFormat: { type: 'price', precision: Math.min(quoteDecimals, data[0] && data[0].close >= 100 ? 2 : 5), minMove: 1 / 10 ** Math.min(quoteDecimals, data[0] && data[0].close >= 100 ? 2 : 5) },
    });
    series.setData(data.map(({ time, open, high, low, close }) => ({ time, open, high, low, close })));
    const vol = chart.addSeries(HistogramSeries, { priceFormat: { type: 'volume' }, priceScaleId: 'vol' });
    chart.priceScale('vol').applyOptions({ scaleMargins: { top: 0.8, bottom: 0 } });
    vol.setData(data.map((d) => ({ time: d.time, value: d.volume, color: d.close >= d.open ? `${up}55` : `${down}55` })));
    chart.timeScale().fitContent();
    return () => chart.remove();
  }, [data, height, canvas, showTable, themeTick, quoteDecimals]);

  const last = data.at(-1);
  const first = data[0];
  const change = last && first ? (last.close - first.open) / first.open : 0;

  return (
    <div className="chart-box">
      <div className="row between" style={{ marginBottom: 6 }}>
        <div className="legend">
          <span>
            <span className="sw" style={{ background: 'var(--up)' }} />
            Up candle
          </span>
          <span>
            <span className="sw" style={{ background: 'var(--down)' }} />
            Down candle
          </span>
          <span className="muted">Volume in {base} below</span>
          {last && (
            <span className={change >= 0 ? 'up' : 'down'}>
              {change >= 0 ? '+' : ''}
              {(change * 100).toFixed(2)}% over range
            </span>
          )}
        </div>
        {canvas && (
          <button type="button" className="btn small" onClick={() => setShowTable((v) => !v)}>
            {showTable ? 'Chart' : 'Table'}
          </button>
        )}
      </div>
      {canvas && !showTable ? (
        <div ref={ref} style={{ width: '100%', height }} role="img" aria-label={`${base}/${quote} candles`} />
      ) : (
        <div className="table-wrap" style={{ maxHeight: height, overflowY: 'auto' }}>
          <table className="tbl">
            <thead>
              <tr>
                <th>Time</th>
                <th className="num">Open</th>
                <th className="num">High</th>
                <th className="num">Low</th>
                <th className="num">Close</th>
                <th className="num">Volume ({base})</th>
              </tr>
            </thead>
            <tbody>
              {[...candles]
                .sort((a, b) => b.t - a.t)
                .slice(0, 200)
                .map((c) => (
                  <tr key={c.t}>
                    <td>{formatExact(c.t * 1000)}</td>
                    <td className="num">{formatPrice(c.o, quoteDecimals)}</td>
                    <td className="num">{formatPrice(c.h, quoteDecimals)}</td>
                    <td className="num">{formatPrice(c.l, quoteDecimals)}</td>
                    <td className={`num ${BigInt(c.c) >= BigInt(c.o) ? 'up' : 'down'}`}>{formatPrice(c.c, quoteDecimals)}</td>
                    <td className="num">{formatAmount(c.v, baseDecimals, { maxFraction: 4 })}</td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
