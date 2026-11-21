/**
 * What stands in front of every page: the landing page until a wallet is
 * connected, a plain "this is for clients" page for an account that is not
 * an attester, and otherwise the client's records handed to the page.
 */
import { useEffect, useState } from 'react';
import { Outlet, useOutletContext } from 'react-router-dom';
import { useAccount, useClient } from '../api/hooks';
import type { AccountRecord, ClientRecord } from '../api/types';
import { DOCS_URL, WALLET_URL } from '../components/Layout';
import { Card, ErrorState, Hex, Loading, Notice } from '../components/ui';
import { spendable } from '../lib/amounts';
import { planStatus, type Funds, type PlanStatus } from '../lib/plan';
import { useNetwork } from '../network/NetworkContext';
import { explorerUrl } from '../network/networks';
import { useWallet } from '../wallet/WalletContext';
import { LandingPage } from './Landing';

export interface Harbor {
  address: string;
  client: ClientRecord;
  account: AccountRecord;
  /** Spendable balances of the billing asset and KEEL. */
  funds: Funds;
  status: PlanStatus;
  /** Wall clock in seconds, refreshed twice a minute; the chain's block time runs with it. */
  nowSecs: number;
}

export function useHarbor(): Harbor {
  return useOutletContext<Harbor>();
}

function useNowSecs(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function NotAClient({ address }: { address: string }) {
  const { network } = useNetwork();
  return (
    <div className="stack narrow">
      <Card title={<h1>Harbor is for clients</h1>}>
        <div className="prose">
          <p>
            The connected account <Hex value={address} head={8} tail={6} /> is not a client on {network.name}. Keel Harbor is where exchanges and other businesses that attest their users manage their
            plan, their KEEL lock and their fees; there is nothing here for a personal account.
          </p>
          <p>
            <strong>Becoming a client.</strong> A client is an account that governance has registered as an attester. Onboarding is described in the{' '}
            <a href={DOCS_URL}>client documentation</a>; once the proposal that names your operator account has passed, connect that account here.
          </p>
          <p>
            <strong>Looking for your own balances?</strong> They are in <a href={WALLET_URL}>Keel Wallet</a>, and every account is public in the{' '}
            <a href={explorerUrl(network.id, `/account/${address}`)}>explorer</a>.
          </p>
        </div>
      </Card>
    </div>
  );
}

export function ClientGate() {
  const w = useWallet();
  const { network } = useNetwork();
  const address = w.account?.address;
  const client = useClient(address);
  const account = useAccount(address);
  const nowSecs = useNowSecs();
  if (!address) return <LandingPage />;
  if (client.isError) return <ErrorState error={client.error} what="client record" />;
  if (account.isError) return <ErrorState error={account.error} what="account" />;
  if (!client.data || !account.data) {
    return (
      <Card>
        <Loading rows={5} />
      </Card>
    );
  }
  if (!client.data.attester) return <NotAClient address={address} />;
  const funds: Funds = { usdt: spendable(account.data.balances, client.data.billing_asset), keel: spendable(account.data.balances, 'KEEL') };
  const harbor: Harbor = { address, client: client.data, account: account.data, funds, status: planStatus(client.data), nowSecs };
  return (
    <div className="stack">
      {w.account && w.account.network !== network.id && (
        <Notice tone="warn" title="The wallet is on another network.">
          Keel Wallet is connected on {w.account.network}; this page shows {network.name}. Switch one of them before signing.
        </Notice>
      )}
      <Outlet context={harbor} />
    </div>
  );
}
