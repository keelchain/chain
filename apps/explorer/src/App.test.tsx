import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { App, makeQueryClient } from './App';
import { MockExplorerApi } from './api/mock';
import type { Network } from './network/networks';

const networks: Network[] = [
  { id: 'testnet', name: 'Testnet', api: 'mock://testnet' },
  { id: 'mainnet', name: 'Mainnet', api: 'mock://mainnet' },
];
const apis = new Map<string, MockExplorerApi>();
const apiFactory = (n: Network) => {
  let a = apis.get(n.id);
  if (!a) {
    a = new MockExplorerApi(n.id, { latencyMs: 0, live: false, now: 1_788_842_973_244 });
    apis.set(n.id, a);
  }
  return a;
};

function mount(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App networks={networks} apiFactory={apiFactory} queryClient={makeQueryClient()} />
    </MemoryRouter>,
  );
}

describe('explorer pages', () => {
  it('redirects / to the stored network and renders the home stats', async () => {
    mount('/');
    await waitFor(() => expect(screen.getAllByText(/Keelchain/i).length).toBeGreaterThan(0));
    await waitFor(() => expect(screen.getByText(/Block time/i)).toBeTruthy());
  });

  it('renders the block list, an account, markets, offers, vaults and governance from the mock', async () => {
    const api = apiFactory(networks[0]!);
    const health = await api.health();
    mount(`/testnet/blocks/${health.indexed_height}`);
    await waitFor(() => expect(screen.getAllByText(String(health.indexed_height).replace(/\B(?=(\d{3})+(?!\d))/g, ',')).length).toBeGreaterThan(0));

    for (const p of ['/testnet/markets', '/testnet/offers', '/testnet/vaults', '/testnet/governance', '/testnet/validators', '/testnet/assets', '/testnet/txs']) {
      const { unmount } = mount(p);
      await waitFor(() => expect(document.querySelector('table, .tiles, .card')).toBeTruthy());
      unmount();
    }
  });

  it('shows the Lightning card under the vaults from the mock', async () => {
    const api = apiFactory(networks[0]!);
    const ln = await api.lightning();
    mount('/testnet/vaults');
    await waitFor(() => expect(screen.getByText('Lightning (BTC)')).toBeTruthy());
    await waitFor(() => expect(document.querySelector('[data-testid="lightning-pools"]')).toBeTruthy());
    const pools = document.querySelector('[data-testid="lightning-pools"]')!;
    expect(pools.querySelectorAll('tbody tr').length).toBe(ln.pools.length);
    // The node id is clipped on screen and complete in the tooltip.
    expect(pools.querySelector(`[title="${ln.pools[0]!.node_id}"]`)).toBeTruthy();
    // Satoshi strings read as BTC with the vault's decimals.
    expect(pools.textContent).toContain('0.03026');
    const payouts = document.querySelector('[data-testid="lightning-payouts"]')!;
    expect(payouts.querySelectorAll('tbody tr').length).toBe(ln.assignments.length);
    expect(payouts.textContent).toContain(`#${ln.assignments[0]!.outbound_id}`);
    expect(screen.getByText(/No excess is being swept/)).toBeTruthy();
  });

  it('shows an unknown-network page for a bad prefix', async () => {
    mount('/nope/blocks');
    await waitFor(() => expect(screen.getByText(/unknown network|not configured/i)).toBeTruthy());
  });
});
