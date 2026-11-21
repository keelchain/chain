/**
 * Reading an order book for a market buy: what a budget of the quote asset
 * buys right now. Prices are quote units per whole base coin and sizes are
 * base units, as the node reports them.
 */
import type { Book, BookLevel } from '../api/types';

const BASE_UNIT = 1_000_000n;
/** Quote units too small to matter: what integer division leaves of a budget. */
const DUST = 1_000n;

function num(v: unknown): bigint | null {
  if (typeof v === 'bigint') return v;
  if (typeof v === 'number' && Number.isSafeInteger(v) && v >= 0) return BigInt(v);
  if (typeof v === 'string' && /^\d+$/.test(v)) return BigInt(v);
  return null;
}

export interface Ask {
  price: bigint;
  size: bigint;
  /** True for the house maker's quote, false for resting orders. */
  house: boolean;
}

/** Everything on offer, cheapest first: the resting asks and the house maker's quote. */
export function asksOf(book: Book | undefined): Ask[] {
  if (!book) return [];
  const out: Ask[] = [];
  for (const l of book.asks ?? ([] as BookLevel[])) {
    const price = num(l.price);
    const size = num(l.size);
    if (price !== null && size !== null && price > 0n && size > 0n) out.push({ price, size, house: false });
  }
  const h = book.house?.ask;
  if (Array.isArray(h)) {
    const price = num(h[0]);
    const size = num(h[1]);
    if (price !== null && size !== null && price > 0n && size > 0n) out.push({ price, size, house: true });
  }
  return out.sort((a, b) => (a.price < b.price ? -1 : a.price > b.price ? 1 : 0));
}

export interface BuyQuote {
  /** Base units the budget buys. */
  bought: bigint;
  /** Quote units it takes; the rest of the budget stays in the balance. */
  spent: bigint;
  /** Average price paid, quote units per whole base coin; null when nothing fills. */
  average: bigint | null;
  /** The price of the last level reached. */
  worst: bigint | null;
  /** False when the book runs out before the budget does. */
  filled: boolean;
}

/** Walk the asks with `budget` quote units. An estimate: the book can move before the order lands. */
export function quoteBuy(asks: Ask[], budget: bigint): BuyQuote {
  let left = budget;
  let bought = 0n;
  let worst: bigint | null = null;
  for (const a of asks) {
    if (left <= 0n) break;
    const cost = (a.price * a.size) / BASE_UNIT;
    const take = cost <= left ? a.size : (left * BASE_UNIT) / a.price;
    if (take <= 0n) break;
    const pay = cost <= left ? cost : (take * a.price) / BASE_UNIT;
    bought += take;
    left -= pay;
    worst = a.price;
    if (cost > left + pay) break;
  }
  const spent = budget - left;
  return { bought, spent, average: bought > 0n ? (spent * BASE_UNIT) / bought : null, worst, filled: bought > 0n && left < DUST };
}
