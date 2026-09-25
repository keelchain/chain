import { HttpExplorerApi } from './client';
import { MockExplorerApi } from './mock';
import type { ExplorerApi } from './types';
import { isMockEnabled, type Network } from '../network/networks';

const cache = new Map<string, ExplorerApi>();

/** One API instance per network for the life of the page. */
export function apiFor(network: Network, mock: boolean = isMockEnabled()): ExplorerApi {
  const key = `${mock ? 'mock' : 'http'}:${network.id}:${network.api}`;
  let api = cache.get(key);
  if (!api) {
    api = mock ? new MockExplorerApi(network.id) : new HttpExplorerApi(network.api);
    cache.set(key, api);
  }
  return api;
}

/** Test hook: replaces the instance used for a network. */
export function setApiFor(network: Network, api: ExplorerApi, mock: boolean = isMockEnabled()): void {
  cache.set(`${mock ? 'mock' : 'http'}:${network.id}:${network.api}`, api);
}

export type { ExplorerApi };
