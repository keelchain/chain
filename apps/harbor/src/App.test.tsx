import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { KeelProvider } from '@keelchain/sdk';
import { App, makeQueryClient } from './App';
import { MOCK_ADDRESS, MockHarborApi, type Scenario } from './api/mock';
import type { Network } from './network/networks';

const networks: Network[] = [{ id: 'testnet', name: 'Testnet', api: 'mock://testnet/api', rpc: 'mock://testnet/rpc' }];

/** A stand-in for the extension: already connected, records what it is asked to sign. */
function installWallet() {
  const account = { address: MOCK_ADDRESS, network: 'testnet', chainId: 7 };
  const signAction = vi.fn(() => Promise.reject(new Error('rejected in the test wallet')));
  const provider = {
    isKeel: true,
    version: 'test',
    connect: () => Promise.resolve(account),
    disconnect: () => Promise.resolve(),
    getAccount: () => Promise.resolve(account),
    signMessage: () => Promise.reject(new Error('unused')),
    signAction,
    authorizeSession: () => Promise.reject(new Error('unused')),
    on: () => () => {},
  } as unknown as KeelProvider;
  (window as unknown as { keel?: KeelProvider }).keel = provider;
  window.sessionStorage.setItem('keel-harbor-connected', '1');
  // The nonce lookup before signing goes to the node RPC.
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(JSON.stringify({ nonce: 7 }), { status: 200, headers: { 'content-type': 'application/json' } }))));
  return { signAction };
}

function open(path: string, scenario: Scenario) {
  const api = new MockHarborApi(scenario);
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App networks={networks} apiFactory={() => api} queryClient={makeQueryClient()} />
    </MemoryRouter>,
  );
}

afterEach(() => {
  cleanup();
  delete (window as unknown as { keel?: KeelProvider }).keel;
  window.sessionStorage.clear();
  vi.unstubAllGlobals();
});

describe('without a wallet', () => {
  it('shows what Harbor is, the live tiers and a Connect button', async () => {
    open('/testnet', 'active');
    expect(screen.getByRole('heading', { name: 'The client account for Keel' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Connect Keel Wallet' })).toBeTruthy();
    const tier2 = await screen.findByRole('region', { name: 'Tier 2' });
    expect(within(tier2).getByText('1,000')).toBeTruthy();
    expect(within(tier2).getByText('500,000')).toBeTruthy();
    expect(within(tier2).getByText('50,000')).toBeTruthy();
    expect(screen.getAllByRole('region', { name: /^Tier \d$/ })).toHaveLength(3);
    // No client pages in the header until a wallet is connected.
    expect(screen.queryByRole('link', { name: 'Funds' })).toBeNull();
  });
});

describe('a connected account that is not a client', () => {
  it('gets in with the same wallet, sees the plans, and cannot buy before registration', async () => {
    installWallet();
    open('/testnet/plans', 'visitor');
    expect(await screen.findByText('This account is not registered as a client yet.')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'client documentation' }).getAttribute('href')).toBe('https://keelchain.com/docs/exchange/');
    expect(screen.getByRole('link', { name: 'Funds' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: /^Buy tier/ })).toBeNull();
  });
});

describe('overview', () => {
  it('no plan: the next step is the lock tier 1 still needs', async () => {
    installWallet();
    open('/testnet', 'none');
    expect(await screen.findByText('Next step')).toBeTruthy();
    // 60,000 of tier 1's 100,000 KEEL are in the balance but nothing is locked.
    expect(screen.getByRole('link', { name: 'Lock 100,000 KEEL' }).getAttribute('href')).toBe('/testnet/funds');
    expect(screen.getByText('No plan')).toBeTruthy();
    expect(screen.getByText(/No period is running, so there is no included usage/)).toBeTruthy();
    expect(screen.getByRole('meter', { name: 'KEEL locked for tier 1' }).getAttribute('aria-valuenow')).toBe('0');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('active plan: tier, days left, renewal and the three meters', async () => {
    installWallet();
    open('/testnet', 'active');
    expect(await screen.findByText('Active')).toBeTruthy();
    expect(screen.getAllByText(/Tier 2/).length).toBeGreaterThan(0);
    expect(screen.getByText('18')).toBeTruthy();
    expect(screen.getByText('All set')).toBeTruthy();
    // 742 of 1,200 USDT of included usage left; 18,650 of 50,000 actions used; lock met.
    expect(screen.getByRole('meter', { name: 'Included usage left' }).getAttribute('aria-valuenow')).toBe('62');
    expect(screen.getByText('742 USDT of 1,200 USDT')).toBeTruthy();
    expect(screen.getByRole('meter', { name: 'User actions used today' }).getAttribute('aria-valuenow')).toBe('37');
    expect(screen.getByRole('meter', { name: 'KEEL locked for tier 2' }).getAttribute('aria-valuenow')).toBe('100');
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('suspended plan: says so and asks for the missing lock', async () => {
    installWallet();
    open('/testnet', 'suspended');
    expect(await screen.findByText('Suspended')).toBeTruthy();
    expect(screen.getByRole('link', { name: 'Lock 40,000 KEEL' })).toBeTruthy();
    const lock = screen.getByRole('meter', { name: 'KEEL locked for tier 1' });
    expect(lock.getAttribute('aria-valuenow')).toBe('60');
    expect(lock.getAttribute('aria-valuetext')).toContain('Plan suspended');
    expect(screen.getByText(/40,000 KEEL more to lock/)).toBeTruthy();
  });

  it('owed: the amount is an alert that says what it blocks, and funding is the next step', async () => {
    installWallet();
    open('/testnet', 'owed');
    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText('37.5 USDT of usage is owed')).toBeTruthy();
    expect(alert.textContent).toContain('no deposit address or payout is issued');
    expect(screen.getByRole('link', { name: 'Add 37.5 USDT' }).getAttribute('href')).toBe('/testnet/funds');
    expect(screen.getByRole('meter', { name: 'Included usage left' }).getAttribute('aria-valuetext')).toContain('Used up');
  });
});

describe('plans', () => {
  it('shows each tier against the client and what an upgrade will charge before signing', async () => {
    const { signAction } = installWallet();
    open('/testnet/plans', 'active');
    const tier2 = await screen.findByRole('region', { name: 'Tier 2' });
    expect(within(tier2).getByText('Current')).toBeTruthy();
    expect(within(screen.getByRole('region', { name: 'Tier 1' })).getByText(/Below your running plan/)).toBeTruthy();
    // Tier 3 needs 2,000,000 KEEL locked; 500,000 are.
    const tier3 = screen.getByRole('region', { name: 'Tier 3' });
    expect(within(tier3).getByText('Needs 1,500,000 KEEL more locked')).toBeTruthy();
    expect(within(tier3).queryByRole('button')).toBeNull();
    // Renewal: the current choice is selected and saving it again is not offered.
    expect((screen.getByRole('radio', { name: /Tier 2/ }) as HTMLInputElement).checked).toBe(true);
    expect((screen.getByRole('button', { name: 'Save renewal' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('radio', { name: /Do not renew/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Save renewal' }));
    await waitFor(() => expect(signAction).toHaveBeenCalledTimes(1));
    const req = (signAction.mock.calls[0] as unknown as [{ envelope: { action: unknown; nonce: number; signer: string; chain_id: number } }])[0];
    expect(req.envelope).toEqual({ signer: MOCK_ADDRESS, nonce: 7, chain_id: 7, action: { SetPlanRenewal: { tier: 0 } } });
    expect(await screen.findByText('rejected in the test wallet')).toBeTruthy();
  });

  it('buys the tier the lock and balance allow, with the charge spelled out', async () => {
    const { signAction } = installWallet();
    const api = new MockHarborApi('none');
    const c = await api.client(MOCK_ADDRESS);
    const a = await api.account();
    const funded = new MockHarborApi({
      client: { ...c, locked_keel: '100000000000', tier: 1, usage_owed: '2500000' },
      account: { ...a, balances: [{ asset: 'USDT', account_type: 'deposit', balance: '400000000' }] },
    });
    render(
      <MemoryRouter initialEntries={['/testnet/plans']}>
        <App networks={networks} apiFactory={() => funded} queryClient={makeQueryClient()} />
      </MemoryRouter>,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Buy tier 1' }));
    expect(screen.getByText('Usage owed, settled first')).toBeTruthy();
    expect(screen.getByText('Taken from your balance now').nextElementSibling?.textContent).toBe('302.5 USDT');
    expect(screen.getByText('Balance after').nextElementSibling?.textContent).toBe('97.5 USDT');
    fireEvent.click(screen.getByRole('button', { name: 'Sign and pay 302.5 USDT' }));
    await waitFor(() => expect(signAction).toHaveBeenCalledTimes(1));
    const req = (signAction.mock.calls[0] as unknown as [{ envelope: { action: unknown } }])[0];
    expect(req.envelope.action).toEqual({ BuyPlan: { tier: 1 } });
  });
});

describe('funds', () => {
  it('warns before an unlock that would suspend the running plan and needs a confirmation', async () => {
    installWallet();
    open('/testnet/funds', 'active');
    const unlock = (await screen.findByRole('heading', { name: 'Unlock KEEL' })).closest('section')!;
    const input = within(unlock).getByLabelText(/Amount \(KEEL\)/);
    const button = within(unlock).getByRole('button', { name: 'Unlock' }) as HTMLButtonElement;
    fireEvent.change(input, { target: { value: '1' } });
    const warning = within(unlock).getByRole('alert');
    expect(warning.textContent).toContain('This suspends your tier 2 plan');
    expect(button.disabled).toBe(true);
    fireEvent.click(within(unlock).getByRole('checkbox'));
    expect(button.disabled).toBe(false);
    // More than is locked is refused outright.
    fireEvent.change(input, { target: { value: '500001' } });
    expect(button.disabled).toBe(true);
  });
});

describe('usage and business', () => {
  it('lists the five prices, the totals and the billing history', async () => {
    installWallet();
    open('/testnet/usage', 'active');
    expect(await screen.findByText('Dispute ruling')).toBeTruthy();
    expect(screen.getByRole('meter', { name: 'Included usage used' }).getAttribute('aria-valuenow')).toBe('38');
    expect(await screen.findByText('Tier 2 period started')).toBeTruthy();
    expect(screen.getAllByText('From allowance').length).toBe(2);
  });

  it('prefills the retail fees and moves the vault tools under Business', async () => {
    installWallet();
    open('/testnet/business', 'active');
    expect(((await screen.findByLabelText(/P2P escrow release/)) as HTMLInputElement).value).toBe('50');
    expect(screen.getByText('1,284')).toBeTruthy();
    fireEvent.click(screen.getByRole('tab', { name: 'Vaults' }));
    expect(await screen.findByText(/No custody vault is registered/)).toBeTruthy();
  });
});
