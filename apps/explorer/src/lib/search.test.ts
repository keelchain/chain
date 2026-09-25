import { describe, expect, it } from 'vitest';
import type { ExplorerApi } from '../api/types';
import { classifyLocally, pathFor, resolveSearch } from './search';

describe('search', () => {
  it('classifies syntax locally', () => {
    expect(classifyLocally('12345')).toEqual({ kind: 'block', ref: '12345' });
    expect(classifyLocally('offer #7')).toEqual({ kind: 'offer', ref: '7' });
    expect(classifyLocally('btc/kusd')).toEqual({ kind: 'market', ref: 'BTC/KUSD' });
    expect(classifyLocally('btc-kusd')).toEqual({ kind: 'market', ref: 'BTC-KUSD' });
    expect(classifyLocally('')).toBeNull();
    expect(pathFor('tx', 'ab')).toBe('/tx/ab');
  });
  it('prefers the indexer answer and falls back to hex → account', async () => {
    const api = { search: async () => ({ kind: 'trade', ref: '3' }) } as unknown as ExplorerApi;
    expect(await resolveSearch(api, 'anything')).toEqual({ path: '/trades/3' });
    const down = { search: async () => { throw new Error('down'); } } as unknown as ExplorerApi;
    const hex = 'cd'.repeat(32);
    expect(await resolveSearch(down, `0x${hex}`)).toEqual({ path: `/account/${hex}` });
    expect(await resolveSearch(down, '42')).toEqual({ path: '/blocks/42' });
  });
});
