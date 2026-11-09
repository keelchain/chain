import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App, makeQueryClient } from '../App';
import { MockExplorerApi } from '../api/mock';
import type { Network } from '../network/networks';

const networks: Network[] = [{ id: 'testnet', name: 'Testnet', api: 'mock://testnet/api' }];
const api = new MockExplorerApi('testnet', { latencyMs: 0, live: false, now: 1_788_842_973_244 });

/** A mock account that holds a Bitcoin deposit address, and that address. */
async function accountWithBtcAddress(): Promise<{ owner: string; address: string }> {
  for (const v of await api.validators()) {
    const a = await api.account(v.address);
    const btc = a.deposit_addresses.find((d) => d.chain === 'BTC');
    if (btc) return { owner: a.address, address: btc.address };
  }
  throw new Error('no mock account with a BTC deposit address');
}

describe('account deposits tab', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('names the external networks and stages an incoming signet transaction by its confirmations', async () => {
    const { owner, address } = await accountWithBtcAddress();
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes('mempool.space/signet/api/blocks/tip/height')) return new Response('330001');
      if (url.includes(`mempool.space/signet/api/address/${address}/txs`)) {
        return Response.json([
          { txid: 'feed'.repeat(16), status: { confirmed: true, block_height: 330_001, block_time: 1_788_842_000 }, vout: [{ scriptpubkey_address: address, value: 242_365 }] },
        ]);
      }
      // TronGrid and anything else: nothing incoming.
      return Response.json({ data: [], block_header: { raw_data: { number: 1 } } });
    });
    vi.stubGlobal('fetch', fetchMock);
    render(
      <MemoryRouter initialEntries={[`/testnet/account/${owner}?tab=deposits`]}>
        <App networks={networks} apiFactory={() => api} queryClient={makeQueryClient()} />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/Deposits arrive on/)).toBeTruthy());
    expect(screen.getByText(/Deposits arrive on/).textContent).toMatch(/Bitcoin signet/);
    await waitFor(() => expect(screen.getByText('1 of 2 confirmations')).toBeTruthy());
    expect(screen.getByText('confirming')).toBeTruthy();
    expect(screen.getByText('1 processing')).toBeTruthy();
    expect(fetchMock.mock.calls.some(([u]) => String(u).includes('mempool.space/signet'))).toBe(true);
  });
});
