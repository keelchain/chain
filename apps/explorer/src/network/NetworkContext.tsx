import { createContext, useContext, useEffect, useMemo, type ReactNode } from 'react';
import { apiFor, type ExplorerApi } from '../api';
import { NETWORKS, storeNetwork, type Network } from './networks';

interface NetworkContextValue {
  network: Network;
  networks: Network[];
  api: ExplorerApi;
  /** Prefix for in-app links: `/testnet`. */
  base: string;
}

const NetworkContext = createContext<NetworkContextValue | null>(null);

export function NetworkProvider({ network, networks = NETWORKS, api, children }: { network: Network; networks?: Network[]; api?: ExplorerApi; children: ReactNode }) {
  useEffect(() => {
    storeNetwork(network.id);
  }, [network.id]);
  const value = useMemo<NetworkContextValue>(
    () => ({ network, networks, api: api ?? apiFor(network), base: `/${network.id}` }),
    [network, networks, api],
  );
  return <NetworkContext.Provider value={value}>{children}</NetworkContext.Provider>;
}

export function useNetwork(): NetworkContextValue {
  const v = useContext(NetworkContext);
  if (!v) throw new Error('useNetwork outside NetworkProvider');
  return v;
}

/** Builds an in-app path for the current network. */
export function useHref(): (path: string) => string {
  const { base } = useNetwork();
  return (path: string) => `${base}${path.startsWith('/') ? path : `/${path}`}`;
}
