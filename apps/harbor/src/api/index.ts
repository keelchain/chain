import { HttpHarborApi } from './client';
import { MockHarborApi, scenarioFromLocation } from './mock';
import type { HarborApi } from './types';
import { isMockEnabled, rpcUrlFor, type Network } from '../network/networks';

const cache = new Map<string, HarborApi>();

/** One API instance per network for the life of the page. */
export function apiFor(network: Network, mock: boolean = isMockEnabled()): HarborApi {
  const key = `${mock ? 'mock' : 'http'}:${network.id}:${network.api}`;
  let api = cache.get(key);
  if (!api) {
    api = mock ? new MockHarborApi(scenarioFromLocation()) : new HttpHarborApi(rpcUrlFor(network), network.api);
    cache.set(key, api);
  }
  return api;
}

export type { HarborApi };
