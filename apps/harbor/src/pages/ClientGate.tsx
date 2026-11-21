/**
 * What stands in front of every page: the landing page until a wallet is
 * connected, and otherwise the account's records handed to the page, with a
 * note on top while the account is not registered as a client.
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

/**
 * An account that is not a client yet still gets all of Harbor: it can
 * hold the billing asset, lock KEEL and see what each tier would take.
 * The one thing it cannot do before registration is buy.
 */
function NotRegistered({ address }: { address: string }) {
  const { network } = useNetwork();
  return (
    <Notice tone="info" title="This account is not registered as a client yet.">
      You do not need another wallet: <Hex value={address} head={8} tail={6} /> becomes the client account once a governance proposal on {network.name} names it, as the{' '}
      <a href={DOCS_URL}>client documentation</a> describes. Until then you can add funds, lock KEEL and compare plans here; buying a plan and the business tools open with registration. Your personal
      balances stay in <a href={WALLET_URL}>Keel Wallet</a> and the <a href={explorerUrl(network.id, `/account/${address}`)}>explorer</a> as before.
    </Notice>
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
  const funds: Funds = { usdt: spendable(account.data.balances, client.data.billing_asset), keel: spendable(account.data.balances, 'KEEL') };
  const harbor: Harbor = { address, client: client.data, account: account.data, funds, status: planStatus(client.data), nowSecs };
  return (
    <div className="stack">
      {w.account && w.account.network !== network.id && (
        <Notice tone="warn" title="The wallet is on another network.">
          Keel Wallet is connected on {w.account.network}; this page shows {network.name}. Switch one of them before signing.
        </Notice>
      )}
      {!client.data.attester && <NotRegistered address={address} />}
      <Outlet context={harbor} />
    </div>
  );
}
