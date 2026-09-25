import { useMemo, useRef, useState } from 'react';
import type { BookLevel } from '../../api/types';
import { formatAmount, formatPrice } from '../../lib/format';

interface Props {
  bids: BookLevel[];
  asks: BookLevel[];
  baseDecimals: number;
  quoteDecimals: number;
  base: string;
  quote: string;
  height?: number;
}

interface Pt {
  price: number;
  cum: number;
}

function cumulative(levels: BookLevel[], baseDecimals: number, quoteDecimals: number, ascending: boolean): Pt[] {
  const pts = levels
    .map(([p, s]) => ({ price: Number(BigInt(p)) / 10 ** quoteDecimals, size: Number(BigInt(s)) / 10 ** baseDecimals }))
    .sort((a, b) => (ascending ? a.price - b.price : b.price - a.price));
  let cum = 0;
  return pts.map((p) => {
    cum += p.size;
    return { price: p.price, cum };
  });
}

/**
 * Order-book depth: cumulative bid size (left, "up" hue) and ask size (right,
 * "down" hue) as 2px lines over a 10% wash, hairline grid, crosshair tooltip.
 */
export function DepthChart({ bids, asks, baseDecimals, quoteDecimals, base, quote, height = 220 }: Props) {
  const ref = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<{ x: number; side: 'bid' | 'ask'; pt: Pt } | null>(null);
  const width = 600;
  const pad = { l: 8, r: 8, t: 10, b: 24 };

  const model = useMemo(() => {
    const b = cumulative(bids, baseDecimals, quoteDecimals, false); // best bid first, walking down
    const a = cumulative(asks, baseDecimals, quoteDecimals, true); // best ask first, walking up
    if (!b.length && !a.length) return null;
    const minP = Math.min(...b.map((p) => p.price), ...a.map((p) => p.price));
    const maxP = Math.max(...b.map((p) => p.price), ...a.map((p) => p.price));
    const maxC = Math.max(...b.map((p) => p.cum), ...a.map((p) => p.cum), 1e-9);
    const mid = b[0] && a[0] ? (b[0].price + a[0].price) / 2 : (b[0]?.price ?? a[0]?.price ?? 0);
    const x = (p: number) => pad.l + ((p - minP) / (maxP - minP || 1)) * (width - pad.l - pad.r);
    const y = (c: number) => pad.t + (1 - c / maxC) * (height - pad.t - pad.b);
    const path = (pts: Pt[], reverse: boolean) => {
      const ordered = reverse ? [...pts].reverse() : pts;
      let d = '';
      ordered.forEach((p, i) => {
        // Step: horizontal to the next price, then vertical.
        const px = x(p.price);
        const py = y(p.cum);
        if (i === 0) d += `M${px},${py}`;
        else {
          const prev = ordered[i - 1]!;
          d += `L${px},${y(prev.cum)}L${px},${py}`;
        }
      });
      return d;
    };
    const bidLine = path(b, true);
    const askLine = path(a, false);
    const bidArea = b.length ? `${bidLine}L${x(b[0]!.price)},${y(0)}L${x(b[b.length - 1]!.price)},${y(0)}Z` : '';
    const askArea = a.length ? `${askLine}L${x(a[a.length - 1]!.price)},${y(0)}L${x(a[0]!.price)},${y(0)}Z` : '';
    const ticks = [0, 0.5, 1].map((f) => ({ v: maxC * f, y: y(maxC * f) }));
    return { b, a, minP, maxP, maxC, mid, x, y, bidLine, askLine, bidArea, askArea, ticks };
  }, [bids, asks, baseDecimals, quoteDecimals, height]);

  if (!model) return <div className="empty">Empty book</div>;

  const onMove = (e: React.MouseEvent<SVGSVGElement>) => {
    const svg = ref.current;
    if (!svg) return;
    const rect = svg.getBoundingClientRect();
    const px = ((e.clientX - rect.left) / rect.width) * width;
    const price = model.minP + ((px - pad.l) / (width - pad.l - pad.r)) * (model.maxP - model.minP);
    const side: 'bid' | 'ask' = price < model.mid ? 'bid' : 'ask';
    const pts = side === 'bid' ? model.b : model.a;
    // Cumulative size at this price = last level crossed walking away from mid.
    let pt = pts[0];
    for (const p of pts) {
      if (side === 'bid' ? p.price >= price : p.price <= price) pt = p;
      else break;
    }
    if (pt) setHover({ x: px, side, pt });
  };

  const q = (v: number) => formatPrice(BigInt(Math.round(v * 10 ** quoteDecimals)).toString(), quoteDecimals);
  const s = (v: number) => formatAmount(BigInt(Math.round(v * 10 ** baseDecimals)).toString(), baseDecimals, { maxFraction: 4 });

  return (
    <div className="chart-box">
      <svg ref={ref} viewBox={`0 0 ${width} ${height}`} width="100%" height={height} role="img" aria-label={`Order book depth for ${base}/${quote}`} onMouseMove={onMove} onMouseLeave={() => setHover(null)} style={{ display: 'block', overflow: 'visible' }}>
        {model.ticks.map((t) => (
          <g key={t.v}>
            <line x1={pad.l} x2={width - pad.r} y1={t.y} y2={t.y} stroke="var(--grid)" strokeWidth="1" />
            <text x={pad.l + 2} y={t.y - 3} fontSize="10" fill="var(--muted)">
              {s(t.v)} {base}
            </text>
          </g>
        ))}
        <path d={model.bidArea} fill="var(--up)" opacity="0.1" />
        <path d={model.askArea} fill="var(--down)" opacity="0.1" />
        <path d={model.bidLine} fill="none" stroke="var(--up)" strokeWidth="2" strokeLinejoin="round" />
        <path d={model.askLine} fill="none" stroke="var(--down)" strokeWidth="2" strokeLinejoin="round" />
        <line x1={model.x(model.mid)} x2={model.x(model.mid)} y1={pad.t} y2={height - pad.b} stroke="var(--border-strong)" strokeDasharray="0" strokeWidth="1" />
        <text x={model.x(model.mid)} y={height - 8} fontSize="10" textAnchor="middle" fill="var(--text-2)">
          mid {q(model.mid)}
        </text>
        <text x={pad.l} y={height - 8} fontSize="10" fill="var(--muted)">
          {q(model.minP)}
        </text>
        <text x={width - pad.r} y={height - 8} fontSize="10" textAnchor="end" fill="var(--muted)">
          {q(model.maxP)}
        </text>
        {hover && (
          <g>
            <line x1={hover.x} x2={hover.x} y1={pad.t} y2={height - pad.b} stroke="var(--text-2)" strokeWidth="1" />
            <circle cx={model.x(hover.pt.price)} cy={model.y(hover.pt.cum)} r="4" fill={hover.side === 'bid' ? 'var(--up)' : 'var(--down)'} stroke="var(--surface)" strokeWidth="2" />
          </g>
        )}
      </svg>
      {hover && (
        <div className="chart-tooltip" style={{ left: `${(hover.x / width) * 100}%`, top: 8, transform: hover.x > width / 2 ? 'translateX(-105%)' : 'translateX(8px)' }}>
          <div>
            <span className="name">{hover.side === 'bid' ? 'Bids to' : 'Asks to'} </span>
            {q(hover.pt.price)} {quote}
          </div>
          <div>
            <span className="name">Cumulative </span>
            {s(hover.pt.cum)} {base}
          </div>
        </div>
      )}
      <div className="legend" style={{ marginTop: 6 }}>
        <span>
          <span className="sw" style={{ background: 'var(--up)' }} />
          Bids (cumulative {base})
        </span>
        <span>
          <span className="sw" style={{ background: 'var(--down)' }} />
          Asks (cumulative {base})
        </span>
      </div>
    </div>
  );
}
