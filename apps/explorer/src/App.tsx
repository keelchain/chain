import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useMemo } from 'react';
import { Navigate, Route, Routes, useParams } from 'react-router-dom';
import type { ExplorerApi } from './api';
import { Layout } from './components/Layout';
import { NetworkProvider } from './network/NetworkContext';
import { NETWORKS, readStoredNetwork, type Network } from './network/networks';
import { AccountPage } from './pages/Account';
import { AssetPage, AssetsPage } from './pages/Assets';
import { BlockPage, BlocksPage } from './pages/Blocks';
import { GovernancePage, ParamsPage, ProposalPage } from './pages/Governance';
import { HomePage } from './pages/Home';
import { MarketPage, MarketsPage, OrderPage } from './pages/Markets';
import { NotFoundPage, SearchPage, UnknownNetworkPage } from './pages/Misc';
import { OfferPage, OffersPage, TradePage } from './pages/Offers';
import { TxPage, TxsPage } from './pages/Txs';
import { ValidatorsPage } from './pages/Validators';
import { VaultPage, VaultsPage } from './pages/Vaults';

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

function NetworkRoutes({ networks, apiFactory }: { networks: Network[]; apiFactory?: (n: Network) => ExplorerApi }) {
  const { network: id = '' } = useParams();
  const network = networks.find((n) => n.id === id);
  if (!network) return <UnknownNetworkPage id={id} />;
  return (
    <NetworkProvider network={network} networks={networks} api={apiFactory?.(network)}>
      <Routes>
        <Route element={<Layout />}>
          <Route index element={<HomePage />} />
          <Route path="blocks" element={<BlocksPage />} />
          <Route path="blocks/:height" element={<BlockPage />} />
          <Route path="txs" element={<TxsPage />} />
          <Route path="tx/:id" element={<TxPage />} />
          <Route path="txs/:id" element={<TxPage />} />
          <Route path="account/:addr" element={<AccountPage />} />
          <Route path="address/:addr" element={<AccountPage />} />
          <Route path="assets" element={<AssetsPage />} />
          <Route path="assets/:asset" element={<AssetPage />} />
          <Route path="markets" element={<MarketsPage />} />
          <Route path="markets/:pair" element={<MarketPage />} />
          <Route path="orders/:id" element={<OrderPage />} />
          <Route path="offers" element={<OffersPage />} />
          <Route path="offers/:id" element={<OfferPage />} />
          <Route path="trades/:id" element={<TradePage />} />
          <Route path="validators" element={<ValidatorsPage />} />
          <Route path="vaults" element={<VaultsPage />} />
          <Route path="vaults/:chain" element={<VaultPage />} />
          <Route path="governance" element={<GovernancePage />} />
          <Route path="governance/params" element={<ParamsPage />} />
          <Route path="governance/:id" element={<ProposalPage />} />
          <Route path="search" element={<SearchPage />} />
          <Route path="*" element={<NotFoundPage />} />
        </Route>
      </Routes>
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
  apiFactory?: (n: Network) => ExplorerApi;
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
