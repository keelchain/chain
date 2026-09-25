import { useMemo } from 'react';
import { useAssets } from '../api/hooks';
import { KNOWN_DECIMALS } from './format';

/** Decimals table from the indexer's asset list, falling back to known assets. */
export function useDecimals(): (asset: string) => number {
  const { data } = useAssets();
  return useMemo(() => {
    const table: Record<string, number> = { ...KNOWN_DECIMALS };
    for (const a of data ?? []) table[a.asset] = a.decimals;
    return (asset: string) => table[asset] ?? 6;
  }, [data]);
}
