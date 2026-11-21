/**
 * react-query hooks over the current network's API. Query keys start with
 * the network id so switching networks never shows another network's data.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { KNOWN_DECIMALS } from '../lib/format';
import { useNetwork } from '../network/NetworkContext';

/** Any address answers with the tiers and prices; this one has no key. */
export const NO_ACCOUNT = '00'.repeat(32);

/** The client record of `addr`: plan, lock, prices, tiers. */
export function useClient(addr: string | undefined) {
  const { api, network } = useNetwork();
  return useQuery({
    queryKey: [network.id, 'client', addr],
    queryFn: () => api.client(addr!),
    enabled: !!addr,
    refetchInterval: 10_000,
  });
}

/** The order book of `pair`, refreshed every few seconds while it is on screen. */
export function useBook(pair: string) {
  const { api, network } = useNetwork();
  return useQuery({ queryKey: [network.id, 'book', pair], queryFn: () => api.book(pair), refetchInterval: 5_000 });
}

/** Tiers, period and prices for a visitor without a wallet. */
export function usePublicTerms() {
  const { api, network } = useNetwork();
  return useQuery({
    queryKey: [network.id, 'client', NO_ACCOUNT],
    queryFn: () => api.client(NO_ACCOUNT),
    staleTime: 60_000,
  });
}

export function useAccount(addr: string | undefined) {
  const { api, network } = useNetwork();
  return useQuery({
    queryKey: [network.id, 'account', addr],
    queryFn: () => api.account(addr!),
    enabled: !!addr,
    refetchInterval: 10_000,
  });
}

export function useCustody(addr: string | undefined) {
  const { api, network } = useNetwork();
  return useQuery({
    queryKey: [network.id, 'custody', addr],
    queryFn: () => api.custody(addr!),
    enabled: !!addr,
    refetchInterval: 10_000,
  });
}

export function useUnlockDelay() {
  const { api, network } = useNetwork();
  return useQuery({
    queryKey: [network.id, 'unlock-delay'],
    queryFn: () => api.unlockDelaySecs(),
    staleTime: 300_000,
  });
}

/** The account's transactions from the indexer, newest first, paged. */
export function useHistory(addr: string | undefined) {
  const { api, network } = useNetwork();
  return useInfiniteQuery({
    queryKey: [network.id, 'history', addr],
    queryFn: ({ pageParam }) => api.history(addr!, pageParam),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    enabled: !!addr,
  });
}

/** Decimals per asset: the indexer's asset list over the well-known table. */
export function useDecimals(): (asset: string) => number {
  const { api, network } = useNetwork();
  const { data } = useQuery({ queryKey: [network.id, 'assets'], queryFn: () => api.assets(), staleTime: 300_000 });
  return useMemo(() => {
    const table: Record<string, number> = { ...KNOWN_DECIMALS };
    for (const a of data ?? []) table[a.asset] = a.decimals;
    return (asset: string) => table[asset] ?? 6;
  }, [data]);
}
