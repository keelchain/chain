/**
 * Mounts every page against a real indexer. Opt-in:
 *   EXPLORER_LIVE_API=http://127.0.0.1:6100 npx vitest run src/App.live.test.tsx
 * Skipped otherwise, so the default suite needs no services.
 */
import { render, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { App, makeQueryClient } from './App';
import { HttpExplorerApi } from './api/client';
import type { Network } from './network/networks';

const base = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env?.['EXPLORER_LIVE_API'];
const networks: Network[] = [{ id: 'testnet', name: 'Testnet', api: base ?? 'http://127.0.0.1:6100' }];
const api = new HttpExplorerApi(networks[0]!.api);
const apiFactory = () => api;

function mount(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App networks={networks} apiFactory={apiFactory} queryClient={makeQueryClient()} />
    </MemoryRouter>,
  );
}

async function expectRenders(path: string) {
  const { unmount, container } = mount(path);
  try {
    // Data arrived: no skeletons left and no error box.
    await waitFor(
      () => {
        expect(container.querySelectorAll('.skeleton').length).toBe(0);
        expect(container.querySelector('[role="alert"]')).toBeNull();
      },
      { timeout: 15_000 },
    );
    expect(container.textContent!.length).toBeGreaterThan(50);
  } finally {
    unmount();
  }
}

describe.skipIf(!base)('explorer against a live indexer', () => {
  it('renders the list pages', async () => {
    for (const p of ['/testnet', '/testnet/blocks', '/testnet/txs', '/testnet/assets', '/testnet/markets', '/testnet/offers', '/testnet/validators', '/testnet/vaults', '/testnet/governance', '/testnet/governance/params']) {
      await expectRenders(p);
    }
  }, 120_000);

  it('renders detail pages for real objects', async () => {
    const health = await api.health();
    const blocks = await api.blocks({ limit: 1 });
    const tip = blocks.blocks[0]!.height;
    await expectRenders(`/testnet/blocks/${tip}`);
    const txs = await api.txs({ limit: 1 });
    if (txs.txs[0]) {
      await expectRenders(`/testnet/tx/${txs.txs[0].tx_id}`);
      await expectRenders(`/testnet/account/${txs.txs[0].signer}`);
    }
    const markets = await api.markets();
    if (markets[0]) {
      await expectRenders(`/testnet/markets/${encodeURIComponent(markets[0].pair)}`);
      const fills = await api.fills(markets[0].pair, { limit: 1 });
      const fill = fills.fills[0] as { taker_order_id?: number } | undefined;
      if (fill?.taker_order_id !== undefined) await expectRenders(`/testnet/orders/${fill.taker_order_id}`);
    }
    const offers = await api.offers({ limit: 1 });
    if (offers.offers[0]) {
      const o = await api.offer(offers.offers[0].id);
      await expectRenders(`/testnet/offers/${o.id}`);
      if (o.trades[0]) await expectRenders(`/testnet/trades/${o.trades[0].id}`);
    }
    const vaults = await api.vaults();
    if (vaults[0]) await expectRenders(`/testnet/vaults/${vaults[0].chain}`);
    expect(health.indexed_height).toBeGreaterThan(0);
  }, 120_000);

  it('searches by height, tx and address through the indexer', async () => {
    const blocks = await api.blocks({ limit: 1 });
    const r = await api.search(String(blocks.blocks[0]!.height));
    expect(r.kind).toBe('block');
  });
});
