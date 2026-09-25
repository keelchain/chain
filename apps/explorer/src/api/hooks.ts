/**
 * react-query hooks over the current network's API. Query keys are prefixed
 * with the network id so switching networks never shows stale data.
 */
import { useQuery, useQueryClient, type UseQueryOptions } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { useNetwork } from '../network/NetworkContext';
import type { CandleInterval, ExplorerApi, OffersQuery, TxsQuery, WsMessage } from './types';

type Fn<T> = (api: ExplorerApi) => Promise<T>;

function useApiQuery<T>(key: unknown[], fn: Fn<T>, opts: Partial<UseQueryOptions<T>> = {}) {
  const { api, network } = useNetwork();
  return useQuery<T>({
    queryKey: [network.id, ...key],
    queryFn: () => fn(api),
    ...opts,
  });
}

export const useHealth = () => useApiQuery(['health'], (a) => a.health(), { refetchInterval: 10_000, retry: 1 });
export const useStats = () => useApiQuery(['stats'], (a) => a.stats(), { refetchInterval: 5_000 });
export const useBlocks = (q: { limit?: number; cursor?: string } = {}, live = false) =>
  useApiQuery(['blocks', q], (a) => a.blocks(q), live ? { refetchInterval: 3_000 } : {});
export const useBlock = (height: number) => useApiQuery(['block', height], (a) => a.block(height), { enabled: Number.isFinite(height) });
export const useTxs = (q: TxsQuery = {}, live = false) => useApiQuery(['txs', q], (a) => a.txs(q), live ? { refetchInterval: 3_000 } : {});
export const useTx = (id: string) => useApiQuery(['tx', id], (a) => a.tx(id), { enabled: !!id });
export const useAccount = (addr: string) => useApiQuery(['account', addr], (a) => a.account(addr), { enabled: !!addr });
export const useAccountTxs = (addr: string, q: TxsQuery = {}) => useApiQuery(['account-txs', addr, q], (a) => a.accountTxs(addr, q));
export const useAccountTransfers = (addr: string, q: { limit?: number; cursor?: string } = {}) => useApiQuery(['account-transfers', addr, q], (a) => a.accountTransfers(addr, q));
export const useAccountOrders = (addr: string) => useApiQuery(['account-orders', addr], (a) => a.accountOrders(addr));
export const useAccountOffers = (addr: string) => useApiQuery(['account-offers', addr], (a) => a.accountOffers(addr));
export const useAccountTrades = (addr: string) => useApiQuery(['account-trades', addr], (a) => a.accountTrades(addr));
export const useAssets = () => useApiQuery(['assets'], (a) => a.assets(), { staleTime: 30_000 });
export const useAsset = (asset: string) => useApiQuery(['asset', asset], (a) => a.asset(asset));
export const useMarkets = () => useApiQuery(['markets'], (a) => a.markets(), { refetchInterval: 5_000 });
export const useMarket = (pair: string) => useApiQuery(['market', pair], (a) => a.market(pair), { refetchInterval: 3_000 });
export const useFills = (pair: string, q: { limit?: number; cursor?: string } = {}) => useApiQuery(['fills', pair, q], (a) => a.fills(pair, q), { refetchInterval: 3_000 });
export const useCandles = (pair: string, interval: CandleInterval) => useApiQuery(['candles', pair, interval], (a) => a.candles(pair, interval), { refetchInterval: 15_000 });
export const useOrder = (id: number) => useApiQuery(['order', id], (a) => a.order(id));
export const useOffers = (q: OffersQuery = {}) => useApiQuery(['offers', q], (a) => a.offers(q));
export const useOffer = (id: number) => useApiQuery(['offer', id], (a) => a.offer(id));
export const useTrade = (id: number) => useApiQuery(['trade', id], (a) => a.trade(id));
export const useValidators = () => useApiQuery(['validators'], (a) => a.validators());
export const useEpochs = () => useApiQuery(['epochs'], (a) => a.epochs());
export const useVaults = () => useApiQuery(['vaults'], (a) => a.vaults(), { refetchInterval: 10_000 });
export const useVaultDeposits = (chain: string, status?: string) => useApiQuery(['vault-deposits', chain, status], (a) => a.vaultDeposits(chain, status));
export const useOutbounds = (status?: string) => useApiQuery(['outbounds', status], (a) => a.outbounds(status));
export const useLightning = () => useApiQuery(['lightning'], (a) => a.lightning(), { refetchInterval: 10_000 });
export const useProposals = () => useApiQuery(['proposals'], (a) => a.proposals());
export const useProposal = (id: number) => useApiQuery(['proposal', id], (a) => a.proposal(id));
export const useParams = () => useApiQuery(['params'], (a) => a.params());

/**
 * Subscribes to the live feed for the current network and invalidates the
 * list queries on every new block, so the home page and lists follow the tip
 * without a full poll. Returns the latest block height seen.
 */
export function useLiveBlocks(onMessage?: (m: WsMessage) => void): void {
  const { api, network } = useNetwork();
  const qc = useQueryClient();
  const cb = useRef(onMessage);
  cb.current = onMessage;
  useEffect(() => {
    let pending: ReturnType<typeof setTimeout> | null = null;
    const unsub = api.subscribe((m) => {
      cb.current?.(m);
      if (m.type === 'block' && !pending) {
        // Coalesce bursts: one invalidation per ~700ms.
        pending = setTimeout(() => {
          pending = null;
          void qc.invalidateQueries({ queryKey: [network.id, 'blocks'] });
          void qc.invalidateQueries({ queryKey: [network.id, 'txs'] });
          void qc.invalidateQueries({ queryKey: [network.id, 'stats'] });
        }, 700);
      }
    });
    return () => {
      if (pending) clearTimeout(pending);
      unsub();
    };
  }, [api, network.id, qc]);
}
