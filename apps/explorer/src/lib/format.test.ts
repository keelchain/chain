import { describe, expect, it } from 'vitest';
import { decimalsOf, formatAmount, formatBps, formatRelative, formatUsd, splitPair, truncateHex } from './format';

describe('formatAmount', () => {
  it('scales smallest units by decimals without float loss', () => {
    expect(formatAmount('123456789', 8)).toBe('1.23456789');
    expect(formatAmount('100000000', 8)).toBe('1');
    expect(formatAmount('21000000000000000', 6)).toBe('21,000,000,000');
    expect(formatAmount('340282366920938463463374607431768211455', 18)).toBe('340,282,366,920,938,463,463.37460743');
  });
  it('handles empty, negative and fixed fraction', () => {
    expect(formatAmount(null, 6)).toBe('—');
    expect(formatAmount('-1500000', 6)).toBe('-1.5');
    expect(formatAmount('1500000', 6, { minFraction: 2 })).toBe('1.50');
    expect(formatAmount('1234567000000', 6, { compact: true })).toBe('1.23M');
  });
});

describe('helpers', () => {
  it('formats micro-USD, bps, hex and pairs', () => {
    expect(formatUsd('1234567890')).toContain('1,234.56');
    expect(formatBps(200)).toBe('2%');
    expect(formatBps(15)).toBe('0.15%');
    expect(truncateHex('ced54e07087d13d96890053bd0db9deb8dd3d9a22b2e56d52d06a6b9c4de01e4')).toBe('ced54e07…de01e4');
    expect(splitPair('BTC-KUSD')).toEqual({ base: 'BTC', quote: 'KUSD' });
    expect(splitPair('BTC.BTC/KUSD')).toEqual({ base: 'BTC.BTC', quote: 'KUSD' });
    expect(decimalsOf('BTC')).toBe(8);
    expect(decimalsOf('BTC.BTC')).toBe(8);
    expect(decimalsOf('KUSD')).toBe(6);
  });
  it('relative time reads naturally', () => {
    const now = 1_788_842_973_244;
    expect(formatRelative(now - 5_000, now)).toMatch(/5s ago|just now/);
    expect(formatRelative(now - 3_600_000 * 3, now)).toMatch(/3h/);
  });
});
