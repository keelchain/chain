/**
 * Number, amount and time formatting. Amounts arrive as decimal strings in
 * smallest units; everything here is BigInt-exact until the final display.
 */

export function pow10(n: number): bigint {
  return 10n ** BigInt(n);
}

function groupThousands(intPart: string): string {
  return intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

export interface AmountFormatOptions {
  /** Max fraction digits shown (default: min(decimals, 8)). Trailing zeros trimmed. */
  maxFraction?: number;
  /** Minimum fraction digits kept (default 0; use 2 for fiat-like display). */
  minFraction?: number;
  /** Compact large values: 1.2M, 3.4K (default false). */
  compact?: boolean;
}

/** Formats `raw` smallest units with `decimals` into a human number string. */
export function formatAmount(raw: string | number | bigint | null | undefined, decimals: number, opts: AmountFormatOptions = {}): string {
  if (raw === null || raw === undefined || raw === '') return '—';
  let value: bigint;
  try {
    value = typeof raw === 'bigint' ? raw : BigInt(typeof raw === 'number' ? Math.trunc(raw) : raw.trim());
  } catch {
    return String(raw);
  }
  const negative = value < 0n;
  if (negative) value = -value;
  const scale = pow10(decimals);
  const intPart = value / scale;
  let frac = (value % scale).toString().padStart(decimals, '0');
  const maxFraction = opts.maxFraction ?? Math.min(decimals, 8);
  const minFraction = opts.minFraction ?? 0;
  frac = frac.slice(0, maxFraction);
  frac = frac.replace(/0+$/, '');
  if (frac.length < minFraction) frac = frac.padEnd(minFraction, '0');
  if (opts.compact) {
    const whole = Number(intPart) + (frac ? Number(`0.${frac}`) : 0);
    return `${negative ? '-' : ''}${formatCompact(whole)}`;
  }
  const s = groupThousands(intPart.toString()) + (frac ? `.${frac}` : '');
  return negative ? `-${s}` : s;
}

/** 1234 -> "1,234"; 1234567 -> "1.23M" when compact. */
export function formatCompact(n: number): string {
  const abs = Math.abs(n);
  if (abs >= 1e12) return `${(n / 1e12).toFixed(2)}T`;
  if (abs >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (abs >= 1e6) return `${(n / 1e6).toFixed(2)}M`;
  if (abs >= 1e4) return `${(n / 1e3).toFixed(1)}K`;
  if (Number.isInteger(n)) return groupThousands(String(n));
  return n.toLocaleString('en-US', { maximumFractionDigits: 2 });
}

export function formatInt(n: number | string | null | undefined): string {
  if (n === null || n === undefined) return '—';
  const s = typeof n === 'number' ? String(Math.trunc(n)) : n;
  return groupThousands(s);
}

/** USD micro-units (6 decimals) as "$1,234.56". */
export function formatUsd(micro: string | number | null | undefined, opts: { compact?: boolean } = {}): string {
  if (micro === null || micro === undefined) return '—';
  if (opts.compact) return `$${formatAmount(micro, 6, { compact: true })}`;
  return `$${formatAmount(micro, 6, { maxFraction: 2, minFraction: 2 })}`;
}

export function formatBps(bps: number | string | null | undefined): string {
  if (bps === null || bps === undefined) return '—';
  const n = Number(bps);
  return `${(n / 100).toLocaleString('en-US', { maximumFractionDigits: 2 })}%`;
}

export function formatPercent(fraction: number | null | undefined, digits = 1): string {
  if (fraction === null || fraction === undefined || !Number.isFinite(fraction)) return '—';
  return `${(fraction * 100).toFixed(digits)}%`;
}

/** Price: quote smallest units per whole base unit. */
export function formatPrice(raw: string | null | undefined, quoteDecimals: number): string {
  if (!raw) return '—';
  const whole = Number(BigInt(raw)) / 10 ** quoteDecimals;
  const maxFraction = whole >= 1000 ? 2 : whole >= 1 ? 4 : 6;
  return formatAmount(raw, quoteDecimals, { maxFraction, minFraction: Math.min(2, maxFraction) });
}

/** Truncates a hex id: 0xabcd…1234 (no 0x prefix on KEEL ids). */
export function truncateHex(hex: string | null | undefined, head = 8, tail = 6): string {
  if (!hex) return '—';
  if (hex.length <= head + tail + 1) return hex;
  return `${hex.slice(0, head)}…${hex.slice(-tail)}`;
}

export function truncateMiddle(s: string, head = 10, tail = 8): string {
  return truncateHex(s, head, tail);
}

/** "12s ago", "3m ago", "in 5m". `ts` in ms. */
export function formatRelative(ts: number | null | undefined, now: number = Date.now()): string {
  if (!ts) return '—';
  const diffSec = Math.round((now - ts) / 1000);
  const abs = Math.abs(diffSec);
  if (abs < 5) return 'just now';
  let out: string;
  if (abs < 60) out = `${abs}s`;
  else if (abs < 3600) out = `${Math.floor(abs / 60)}m`;
  else if (abs < 86_400) out = `${Math.floor(abs / 3600)}h`;
  else if (abs < 604_800) out = `${Math.floor(abs / 86_400)}d`;
  else if (abs < 2_592_000) out = `${Math.floor(abs / 604_800)}w`;
  else out = `${Math.floor(abs / 2_592_000)}mo`;
  return diffSec < 0 ? `in ${out}` : `${out} ago`;
}

/** Exact UTC timestamp for tooltips: 2026-09-07 14:03:22 UTC. */
export function formatExact(ts: number | null | undefined): string {
  if (!ts) return '—';
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())} UTC`;
}

export function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min`;
  if (seconds < 86_400) return `${(seconds / 3600).toFixed(seconds % 3600 ? 1 : 0)} h`;
  return `${(seconds / 86_400).toFixed(1)} d`;
}

export function formatMs(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

/** Splits "BTC.BTC/KUSD" into base and quote. */
/**
 * On-chain symbols are `BASE-QUOTE` (`BTC-KUSD`, base asset `BTC.BTC`);
 * `BASE/QUOTE` is accepted too. Asset ids never contain a dash, so the last
 * dash is the separator.
 */
export function splitPair(pair: string): { base: string; quote: string } {
  if (pair.includes('/')) {
    const [base = pair, quote = ''] = pair.split('/');
    return { base, quote };
  }
  const i = pair.lastIndexOf('-');
  if (i <= 0) return { base: pair, quote: '' };
  return { base: pair.slice(0, i), quote: pair.slice(i + 1) };
}

/** Well-known decimals, used only when a response lacks `decimals`. */
export const KNOWN_DECIMALS: Record<string, number> = {
  KEEL: 6,
  KUSD: 6,
  'BTC.BTC': 8,
  'ETH.ETH': 18,
  'ETH.USDT': 6,
  'ETH.USDC': 6,
  'TRON.USDT': 6,
  'TRON.TRX': 6,
};

/** Decimals for an asset id, or for a bare symbol (`BTC` → `BTC.BTC`). */
export function decimalsOf(asset: string, table: Record<string, number> = KNOWN_DECIMALS): number {
  const direct = table[asset];
  if (direct !== undefined) return direct;
  const bySymbol = Object.entries(table).find(([id]) => id.endsWith(`.${asset}`));
  return bySymbol ? bySymbol[1] : 6;
}

/** Human label for an asset: "BTC (Bitcoin vault)". */
export function assetLabel(asset: string): string {
  if (asset === 'KEEL') return 'KEEL';
  if (asset === 'KUSD') return 'KUSD';
  return asset;
}

export function sentence(s: string): string {
  return s.replace(/[_-]+/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2').replace(/^\w/, (c) => c.toUpperCase());
}
