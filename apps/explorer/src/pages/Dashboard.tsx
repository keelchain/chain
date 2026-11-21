/**
 * The dashboard behind the header's Connect button: the connected wallet's
 * own account. A client (an attester) manages its plan, lock, usage, fees
 * and vaults in Keel Harbor, which this page links to.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { Amount, Card, ErrorState, Hex, Loading } from '../components/ui';
import { formatInt } from '../lib/format';
import { useHref, useNetwork } from '../network/NetworkContext';
import { useWallet, type WalletValue } from '../wallet/WalletContext';

/** Where clients manage their account; the network id is the first path segment there too. */
export const HARBOR_URL: string = (import.meta.env.VITE_HARBOR_URL ?? 'https://harbor.keelchain.com').replace(/\/+$/, '');

interface Balance {
  asset: string;
  account_type: string;
  balance: string;
}

/** Decimal text to smallest units for `asset`, or null when malformed. */
export function toUnits(text: string, decimals: number): bigint | null {
  const t = text.trim();
  if (!/^\d+(\.\d+)?$/.test(t)) return null;
  const [whole, frac = ''] = t.split('.');
  if (frac.length > decimals) return null;
  return BigInt(whole!) * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals) || '0');
}

function useOwnAccount(w: WalletValue) {
  const addr = w.account?.address;
  const account = useQuery({
    queryKey: ['wallet-account', addr, w.rpc],
    queryFn: () => w.rpc.account(addr!) as Promise<{ nonce: number; balances?: Balance[] }>,
    enabled: !!addr,
    refetchInterval: 10_000,
  });
  // The node answers for any address; `attester` says whether it is a client.
  const client = useQuery({
    queryKey: ['wallet-client', addr, w.rpc],
    queryFn: () => w.rpc.client(addr!) as Promise<{ attester?: boolean }>,
    enabled: !!addr,
  });
  return { account, isClient: client.data?.attester === true };
}

function AccountSection({ w, account }: { w: WalletValue; account: ReturnType<typeof useOwnAccount>['account'] }) {
  const href = useHref();
  const addr = w.account!.address;
  if (account.isLoading) return <Loading rows={4} />;
  if (account.isError) return <ErrorState error={account.error} what="account" />;
  const balances = account.data?.balances ?? [];
  return (
    <Card title={<h2>Balances</h2>} actions={<Link to={href(`/account/${addr}`)} className="btn small">Explorer view</Link>} flush>
      <div className="table-wrap">
        <table className="tbl">
          <thead>
            <tr>
              <th>Asset</th>
              <th>Account</th>
              <th className="num">Balance</th>
            </tr>
          </thead>
          <tbody>
            {balances.length === 0 && (
              <tr>
                <td colSpan={3} className="text-2">
                  Nothing yet. The faucet funds testnet accounts.
                </td>
              </tr>
            )}
            {balances.map((b) => (
              <tr key={`${b.asset}:${b.account_type}`}>
                <td>{b.asset}</td>
                <td className="text-2">{b.account_type}</td>
                <td className="num">
                  <Amount value={b.balance} asset={b.asset} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="card-body small text-2">
        Address <Hex value={addr} head={10} tail={8} /> · nonce {formatInt(account.data?.nonce ?? 0)}
      </div>
    </Card>
  );
}

export function DashboardPage() {
  const w = useWallet();
  const { network } = useNetwork();
  const own = useOwnAccount(w);
  if (!w.account) {
    return (
      <Card title={<h2>Dashboard</h2>}>
        <p>Connect Keel Wallet to see your account.</p>
        <div className="row">
          <button type="button" className="btn primary" onClick={() => void w.connect()} disabled={w.busy}>
            {w.busy ? 'Connecting…' : 'Connect Keel Wallet'}
          </button>
          {w.detected && !w.provider && (
            <a className="btn" href="https://keelchain.com/wallet/">
              Get the wallet
            </a>
          )}
          {w.error && <span className="small down">{w.error}</span>}
        </div>
      </Card>
    );
  }
  return (
    <div className="dashboard">
      <div className="row" style={{ justifyContent: 'space-between', flexWrap: 'wrap' }}>
        <h1 style={{ margin: 0 }}>Dashboard</h1>
        <span className="small text-2">
          <Hex value={w.account.address} head={8} tail={6} /> on {w.account.network}
        </span>
      </div>
      {own.isClient && (
        <div className="notice info">
          This account is a client. Its plan, KEEL lock, usage, retail fees and vaults are managed in{' '}
          <a href={`${HARBOR_URL}/${network.id}`} target="_blank" rel="noreferrer noopener">
            Keel Harbor ↗
          </a>
          .
        </div>
      )}
      <AccountSection w={w} account={own.account} />
    </div>
  );
}
