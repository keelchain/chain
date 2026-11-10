import { useEffect, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { useAccount } from '../api/hooks';
import type { Balance } from '../api/types';
import { useHref } from '../network/NetworkContext';
import { useWallet } from '../wallet/WalletContext';
import { Amount, Hex } from './ui';

/** Spendable balance of `asset` from a balances list, as a display string
 *  (used by the dashboard, which reads the node RPC for the nonce as well). */
export function spendable(balances: { asset: string; account_type: string; balance: string }[] | undefined, asset: string, decimals: number): string {
  const row = balances?.find((b) => b.asset === asset && b.account_type === 'deposit');
  if (!row) return '0';
  const n = BigInt(row.balance);
  const d = 10n ** BigInt(decimals);
  const whole = n / d;
  const frac = (n % d).toString().padStart(decimals, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : whole.toString();
}

/** The `deposit` rows a user can spend, one per asset, largest first; the
 *  same rows the account page shows, from the indexer (`/v1/accounts/{addr}`). */
export function spendableRows(balances: Balance[] | undefined): Balance[] {
  return (balances ?? [])
    .filter((b) => b.account_type === 'deposit' && BigInt(b.balance) > 0n)
    .sort((a, b) => a.asset.localeCompare(b.asset));
}

/** Header control: Connect, or the connected address with a drop-down to
 *  the dashboard. */
export function ConnectControl() {
  const w = useWallet();
  const href = useHref();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [open]);
  // Balances come from the indexer, the same source as the account page, so
  // the two never disagree and the dropdown works wherever the page does.
  const acct = useAccount(w.account?.address ?? '');
  if (!w.account) {
    return (
      <button type="button" className="btn primary" onClick={() => void w.connect()} disabled={w.busy} title={w.error ?? 'Connect Keel Wallet'}>
        {w.busy ? 'Connecting…' : 'Connect'}
      </button>
    );
  }
  return (
    <div className="wallet-menu" ref={ref}>
      <button type="button" className="btn" onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open}>
        <span className="network-dot" aria-hidden /> <Hex value={w.account.address} head={6} tail={4} copy={false} />
      </button>
      {open && (
        <div className="wallet-dropdown" role="menu">
          {acct.isError ? (
            <div className="wallet-row"><span className="text-2 small">Balances unavailable</span></div>
          ) : acct.isPending ? (
            <div className="wallet-row"><span className="text-2 small">Loading…</span></div>
          ) : spendableRows(acct.data?.balances).length === 0 ? (
            <div className="wallet-row"><span className="text-2 small">No balances</span></div>
          ) : (
            spendableRows(acct.data?.balances).map((b) => (
              <div className="wallet-row" key={b.asset}>
                <span className="text-2 small">{b.asset}</span>
                <span className="num"><Amount value={b.balance} asset={b.asset} decimals={b.decimals} sym={false} /></span>
              </div>
            ))
          )}
          <Link to={href('/dashboard')} className="btn primary" role="menuitem" onClick={() => setOpen(false)}>
            Dashboard
          </Link>
          <Link to={href(`/account/${w.account.address}`)} className="btn" role="menuitem" onClick={() => setOpen(false)}>
            Explorer view
          </Link>
          <button type="button" className="btn" role="menuitem" onClick={() => void w.disconnect().then(() => setOpen(false))}>
            Disconnect
          </button>
        </div>
      )}
    </div>
  );
}
