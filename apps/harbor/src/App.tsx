import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useMemo } from 'react';
import { Navigate, Route, Routes, useParams } from 'react-router-dom';
import type { HarborApi } from './api';
import { Layout } from './components/Layout';
import { NetworkProvider } from './network/NetworkContext';
import { NETWORKS, readStoredNetwork, type Network } from './network/networks';
import { BusinessPage } from './pages/Business';
import { ClientGate } from './pages/ClientGate';
import { FundsPage } from './pages/Funds';
import { OverviewPage } from './pages/Overview';
import { PlansPage } from './pages/Plans';
import { UsagePage } from './pages/Usage';
import { WalletProvider } from './wallet/WalletContext';

export function makeQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        retry: (count, err) => {
          const status = (err as { status?: number } | undefined)?.status;
          return status !== 404 && count < 1;
        },
        staleTime: 2_000,
        refetchOnWindowFocus: false,
      },
    },
  });
}

function UnknownNetwork({ id, networks }: { id: string; networks: Network[] }) {
  return (
    <div className="main">
      <div className="error-box" role="alert">
        <strong>Unknown network “{id}”.</strong> Harbor is set up for:{' '}
        {networks.map((n, i) => (
          <span key={n.id}>
            {i > 0 && ', '}
            <a href={`${import.meta.env.BASE_URL}${n.id}`}>{n.name}</a>
          </span>
        ))}
        .
      </div>
    </div>
  );
}

function NetworkRoutes({ networks, apiFactory }: { networks: Network[]; apiFactory?: (n: Network) => HarborApi }) {
  const { network: id = '' } = useParams();
  const network = networks.find((n) => n.id === id);
  if (!network) return <UnknownNetwork id={id} networks={networks} />;
  return (
    <NetworkProvider network={network} networks={networks} api={apiFactory?.(network)}>
      <WalletProvider>
        <Routes>
          <Route element={<Layout />}>
            {/* Every page is the client's own; the gate shows the landing page until a client's wallet is connected. */}
            <Route element={<ClientGate />}>
              <Route index element={<OverviewPage />} />
              <Route path="plans" element={<PlansPage />} />
              <Route path="funds" element={<FundsPage />} />
              <Route path="usage" element={<UsagePage />} />
              <Route path="business" element={<BusinessPage />} />
              <Route path="business/:tab" element={<BusinessPage />} />
              <Route path="*" element={<Navigate to={`/${network.id}`} replace />} />
            </Route>
          </Route>
        </Routes>
      </WalletProvider>
    </NetworkProvider>
  );
}

function RootRedirect({ networks }: { networks: Network[] }) {
  const target = useMemo(() => readStoredNetwork(networks), [networks]);
  return <Navigate to={`/${target.id}`} replace />;
}

export interface AppProps {
  networks?: Network[];
  queryClient?: QueryClient;
  /** Test hook: supply the API per network instead of the env-driven factory. */
  apiFactory?: (n: Network) => HarborApi;
}

/** Routes are mounted under `/:network/...`; `/` redirects to the stored network. */
export function App({ networks = NETWORKS, queryClient, apiFactory }: AppProps) {
  const qc = useMemo(() => queryClient ?? makeQueryClient(), [queryClient]);
  return (
    <QueryClientProvider client={qc}>
      <Routes>
        <Route path="/" element={<RootRedirect networks={networks} />} />
        <Route path="/:network/*" element={<NetworkRoutes networks={networks} apiFactory={apiFactory} />} />
      </Routes>
    </QueryClientProvider>
  );
}
