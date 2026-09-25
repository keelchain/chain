/** Amount and label formatting (BigInt-exact, mirrors apps/explorer/src/lib/format.ts). */

export const KNOWN_DECIMALS: Record<string, number> = {
  KEEL: 6,
  KUSD: 6,
  'BTC.BTC': 8,
  'ETH.ETH': 18,
  'ETH.USDT': 6,
  'TRON.USDT': 6,
  'TRON.TRX': 6,
};

/** Decimals for an asset id, or for a bare symbol (`BTC` → `BTC.BTC`); 6 when unknown. */
export function decimalsOf(asset: string, table: Record<string, number> = KNOWN_DECIMALS): number {
  const direct = table[asset];
  if (direct !== undefined) return direct;
  const bySymbol = Object.entries(table).find(([id]) => id.endsWith(`.${asset}`));
  return bySymbol ? bySymbol[1] : 6;
}

export function pow10(n: number): bigint {
  return 10n ** BigInt(n);
}

function groupThousands(intPart: string): string {
  return intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** Formats `raw` smallest units with `decimals` into a human number string. Non-numeric input is returned as-is. */
export function formatAmount(raw: string | number | bigint | null | undefined, decimals: number, maxFraction = Math.min(decimals, 8)): string {
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
  let frac = (value % scale).toString().padStart(decimals, '0').slice(0, maxFraction).replace(/0+$/, '');
  const s = groupThousands(intPart.toString()) + (frac ? `.${frac}` : '');
  return negative ? `-${s}` : s;
}

/** `formatAmount` with the asset's known decimals and the symbol appended: "0.05 BTC.BTC". */
export function formatAsset(raw: string | number | bigint | null | undefined, asset: string): string {
  return `${formatAmount(raw, decimalsOf(asset))} ${asset}`;
}

/** Pair symbols are `BASE-QUOTE` (`BTC-KUSD`); `BASE/QUOTE` is accepted too. Asset ids never contain a dash. */
export function splitPair(pair: string): { base: string; quote: string } {
  if (pair.includes('/')) {
    const [base = pair, quote = ''] = pair.split('/');
    return { base, quote };
  }
  const i = pair.lastIndexOf('-');
  if (i <= 0) return { base: pair, quote: '' };
  return { base: pair.slice(0, i), quote: pair.slice(i + 1) };
}

export function truncateHex(hex: string | null | undefined, head = 8, tail = 6): string {
  if (!hex) return '';
  return hex.length <= head + tail + 1 ? hex : `${hex.slice(0, head)}…${hex.slice(-tail)}`;
}

export function sentence(s: string): string {
  return s.replace(/[_-]+/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2').replace(/^\w/, (c) => c.toUpperCase());
}

export function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} min`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} h`;
  return `${Math.round(seconds / 86400)} d`;
}

/** Local date+time in plain words, e.g. "9 Sep 2026, 14:05". */
export function formatDate(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  if (Number.isNaN(d.getTime())) return String(unixSeconds);
  return d.toLocaleString(undefined, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
}
