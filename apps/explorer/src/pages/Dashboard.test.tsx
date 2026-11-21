import { render, screen, waitFor } from '@testing-library/react';
import type { KeelProvider } from '@keelchain/sdk';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App, makeQueryClient } from '../App';
import { MockExplorerApi } from '../api/mock';
import type { Network } from '../network/networks';
import { rpcUrlFor } from '../network/networks';
import { toUnits } from './Dashboard';

const networks: Network[] = [{ id: 'testnet', name: 'Testnet', api: 'mock://testnet/api' }];
const api = new MockExplorerApi('testnet', { latencyMs: 0, live: false, now: 1_788_842_973_244 });

/** A connected wallet, and a node RPC that says whether its account is a client. */
function connect(attester: boolean) {
  const account = { address: 'c1'.repeat(32), network: 'testnet', chainId: 7 };
  const provider = { isKeel: true, version: 'test', getAccount: () => Promise.resolve(account), on: () => () => {} } as unknown as KeelProvider;
  (window as unknown as { keel?: KeelProvider }).keel = provider;
  window.sessionStorage.setItem('keel-dashboard-connected', '1');
  vi.stubGlobal(
    'fetch',
    vi.fn((input: RequestInfo | URL) => Promise.resolve(Response.json(String(input).includes('/v1/clients/') ? { attester } : { nonce: 3, balances: [{ asset: 'KEEL', account_type: 'deposit', balance: '5000000' }] }))),
  );
}

function open() {
  return render(
    <MemoryRouter initialEntries={['/testnet/dashboard']}>
      <App networks={networks} apiFactory={() => api} queryClient={makeQueryClient()} />
    </MemoryRouter>,
  );
}

describe('dashboard', () => {
  afterEach(() => {
    delete (window as unknown as { keel?: KeelProvider }).keel;
    window.sessionStorage.clear();
    vi.unstubAllGlobals();
  });

  it('shows a connected account its balances and nothing about clients', async () => {
    connect(false);
    open();
    expect(await screen.findByRole('heading', { name: 'Balances' })).toBeTruthy();
    await waitFor(() => expect(screen.getByText(/nonce 3/)).toBeTruthy());
    expect(screen.queryByRole('link', { name: /Keel Harbor/ })).toBeNull();
    expect(screen.queryByRole('tab')).toBeNull();
  });

  it('points a connected client to Keel Harbor on the same network', async () => {
    connect(true);
    open();
    const link = await screen.findByRole('link', { name: /Keel Harbor/ });
    expect(link.getAttribute('href')).toBe('https://harbor.keelchain.com/testnet');
    expect(screen.queryByRole('tab')).toBeNull();
  });

  it('asks to connect the wallet when none is installed and shows Connect in the header', async () => {
    render(
      <MemoryRouter initialEntries={['/testnet/dashboard']}>
        <App networks={networks} apiFactory={() => api} queryClient={makeQueryClient()} />
      </MemoryRouter>,
    );
    await waitFor(() => expect(screen.getByText(/Connect Keel Wallet to see your account/i)).toBeTruthy());
    expect(screen.getAllByRole('button', { name: /^Connect/ }).length).toBeGreaterThan(0);
  });

  it('derives the node rpc from the indexer url and converts decimals to units', () => {
    expect(rpcUrlFor({ id: 'testnet', name: 't', api: 'https://testnet.keelchain.com/api' })).toBe('https://testnet.keelchain.com/rpc');
    expect(rpcUrlFor({ id: 'testnet', name: 't', api: 'http://127.0.0.1:6100', rpc: 'http://127.0.0.1:5100' })).toBe('http://127.0.0.1:5100');
    expect(toUnits('1.5', 8)).toBe(150_000_000n);
    expect(toUnits('0.000001', 6)).toBe(1n);
    expect(toUnits('1.1234567', 6)).toBeNull();
    expect(toUnits('abc', 6)).toBeNull();
  });
});
