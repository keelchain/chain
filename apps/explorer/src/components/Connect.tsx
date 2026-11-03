import { useQuery } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { useHref } from '../network/NetworkContext';
import { useWallet } from '../wallet/WalletContext';
import { Hex } from './ui';

interface Balance {
  asset: string;
  account_type: string;
  balance: string;
}

/** Spendable balance of `asset` from `/v1/accounts/{addr}`, as a display string. */
export function spendable(balances: Balance[] | undefined, asset: string, decimals: number): string {
  const row = balances?.find((b) => b.asset === asset && b.account_type === 'deposit');
  if (!row) return '0';
  const n = BigInt(row.balance);
  const d = 10n ** BigInt(decimals);
  const whole = n / d;
  const frac = (n % d).toString().padStart(decimals, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : whole.toString();
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
  const acct = useQuery({
    queryKey: ['wallet-account', w.account?.address, w.rpc],
    queryFn: () => w.rpc.account(w.account!.address) as Promise<{ balances?: Balance[] }>,
    enabled: !!w.account,
    refetchInterval: 10_000,
  });
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
          <div className="wallet-row">
            <span className="text-2 small">KEEL</span>
            <span className="num">{spendable(acct.data?.balances, 'KEEL', 6)}</span>
          </div>
          <div className="wallet-row">
            <span className="text-2 small">KUSD</span>
            <span className="num">{spendable(acct.data?.balances, 'KUSD', 6)}</span>
          </div>
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
