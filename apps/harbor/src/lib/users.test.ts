import { describe, expect, it } from 'vitest';
import { chainOf, payoutRails } from './users';

describe('users', () => {
  it('names a chain from its code or its name', () => {
    expect(chainOf('BTC')).toBe('Bitcoin');
    expect(chainOf('Bsc')).toBe('Bsc');
    expect(chainOf('DOGE')).toBeNull();
  });
  it('offers networks only for the unified dollar balance', () => {
    expect(payoutRails('USDT', 'USDT')).toEqual(['TRON.USDT', 'BSC.USDT', 'ETH.USDT']);
    expect(payoutRails('BTC.BTC', 'USDT')).toBeNull();
  });
});
