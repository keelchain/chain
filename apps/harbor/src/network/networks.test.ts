import { describe, expect, it } from 'vitest';
import { DEFAULT_NETWORKS, parseNetworks } from './networks';

describe('parseNetworks', () => {
  it('accepts a JSON list with id/name/api and rejects bad entries', () => {
    const list = parseNetworks('[{"id":"testnet","name":"Testnet","api":"https://api-testnet.example"},{"id":"Main Net","api":"x"},{"id":"mainnet","api":"https://api.example"}]');
    expect(list.map((n) => n.id)).toEqual(['testnet', 'mainnet']);
    expect(list[1]!.name).toBe('mainnet');
  });
  it('falls back to defaults on garbage', () => {
    expect(parseNetworks(undefined)).toBe(DEFAULT_NETWORKS);
    expect(parseNetworks('not json')).toBe(DEFAULT_NETWORKS);
    expect(parseNetworks('[]')).toBe(DEFAULT_NETWORKS);
  });
});
