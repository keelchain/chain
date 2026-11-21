import { describe, expect, it } from 'vitest';
import { big, fromUnits, keel, money, ratio, shortfall, spendable, toUnits } from './amounts';

describe('amounts', () => {
  it('converts decimal text to smallest units exactly, or refuses', () => {
    expect(toUnits('1.5', 8)).toBe(150_000_000n);
    expect(toUnits('0.000001', 6)).toBe(1n);
    expect(toUnits('100000', 6)).toBe(100_000_000_000n);
    expect(toUnits('1.1234567', 6)).toBeNull();
    expect(toUnits('abc', 6)).toBeNull();
    expect(toUnits('', 6)).toBeNull();
    expect(toUnits('-1', 6)).toBeNull();
  });
  it('round-trips through the input text', () => {
    expect(fromUnits(40_000_000_000n, 6)).toBe('40000');
    expect(fromUnits(1_500_001n, 6)).toBe('1.500001');
    expect(toUnits(fromUnits(123_456_789n, 6), 6)).toBe(123_456_789n);
  });
  it('reads chain amounts tolerantly', () => {
    expect(big('340282366920938463463374607431768211455')).toBe(340282366920938463463374607431768211455n);
    expect(big(12)).toBe(12n);
    expect(big(undefined)).toBe(0n);
    expect(big('nope')).toBe(0n);
  });
  it('keeps ratios in [0, 1] and shortfalls at or above zero', () => {
    expect(ratio(742n, 1_200n)).toBeCloseTo(0.6183, 4);
    expect(ratio(5n, 0n)).toBe(0);
    expect(ratio(9n, 3n)).toBe(1);
    expect(shortfall(100n, 60n)).toBe(40n);
    expect(shortfall(60n, 100n)).toBe(0n);
  });
  it('labels KEEL from orcas and money from USD micro', () => {
    expect(keel(100_000_000_000n)).toBe('100,000 KEEL');
    expect(money('37500000', 'USDT')).toBe('37.5 USDT');
  });
  it('takes the spendable balance from the deposit row only', () => {
    const balances = [
      { asset: 'USDT', account_type: 'deposit', balance: '5' },
      { asset: 'USDT', account_type: 'escrow', balance: '9' },
    ];
    expect(spendable(balances, 'USDT')).toBe(5n);
    expect(spendable(balances, 'KEEL')).toBe(0n);
    expect(spendable(undefined, 'KEEL')).toBe(0n);
  });
});
