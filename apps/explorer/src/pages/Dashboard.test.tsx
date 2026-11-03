import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { App, makeQueryClient } from '../App';
import { MockExplorerApi } from '../api/mock';
import type { Network } from '../network/networks';
import { rpcUrlFor } from '../network/networks';
import { toUnits } from './Dashboard';

const networks: Network[] = [{ id: 'testnet', name: 'Testnet', api: 'mock://testnet/api' }];
const api = new MockExplorerApi('testnet', { latencyMs: 0, live: false, now: 1_788_842_973_244 });

describe('dashboard', () => {
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
