import { describe, expect, it } from 'vitest';
import { asksOf, quoteBuy } from './book';

const USD = 1_000_000n;
const KEEL = 1_000_000n;
const book = {
  symbol: 'KEEL-USDT',
  asks: [
    { price: '500000', size: String(100n * KEEL) },
    { price: '400000', size: String(50n * KEEL) },
  ],
  bids: [],
  house: { ask: ['600000', String(1000n * KEEL)] as [string, string] },
};

describe('asksOf', () => {
  it('lists resting asks and the house quote, cheapest first', () => {
    expect(asksOf(book).map((a) => [a.price, a.house])).toEqual([[400000n, false], [500000n, false], [600000n, true]]);
    expect(asksOf(undefined)).toEqual([]);
    expect(asksOf({ symbol: 'x', asks: [], bids: [], house: null })).toEqual([]);
  });
});

describe('quoteBuy', () => {
  const asks = asksOf(book);
  it('fills inside the first level', () => {
    const q = quoteBuy(asks, 10n * USD);
    expect(q).toMatchObject({ bought: 25n * KEEL, spent: 10n * USD, average: 400000n, worst: 400000n, filled: true });
  });
  it('walks levels and averages the price', () => {
    // 50 KEEL for 20 USDT, then 20 USDT more at 0.5 = 40 KEEL.
    const q = quoteBuy(asks, 40n * USD);
    expect(q.bought).toBe(90n * KEEL);
    expect(q.spent).toBe(40n * USD);
    expect(q.worst).toBe(500000n);
    expect(q.average).toBe((40n * USD * KEEL) / (90n * KEEL));
  });
  it('says when the book runs out before the budget', () => {
    const q = quoteBuy(asksOf({ ...book, house: null }), 1_000n * USD);
    expect(q.bought).toBe(150n * KEEL);
    expect(q.spent).toBe(70n * USD);
    expect(q.filled).toBe(false);
  });
  it('buys nothing from an empty book', () => {
    expect(quoteBuy([], 5n * USD)).toMatchObject({ bought: 0n, spent: 0n, average: null, filled: false });
  });
});
