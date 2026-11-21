/**
 * The connected Keel Wallet: detect the `window.keel` provider the
 * extension injects, connect, remember the session, and sign and submit
 * actions through it. The page never holds a key; every action is signed in
 * the wallet and submitted to the network's node RPC.
 */
import { RpcClient, detectProvider, type AccountInfo, type KeelProvider, type Receipt, type SignedAction } from '@keelchain/sdk';
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { MOCK_ADDRESS } from '../api/mock';
import { useNetwork } from '../network/NetworkContext';
import { isMockEnabled, rpcUrlFor } from '../network/networks';

export interface WalletValue {
  provider: KeelProvider | null;
  account: AccountInfo | null;
  /** The provider was looked for (so "not installed" is a fact, not a race). */
  detected: boolean;
  busy: boolean;
  error: string | null;
  connect(): Promise<void>;
  disconnect(): Promise<void>;
  /** Sign `action` in the wallet, submit it, wait for the receipt. */
  signAndSubmit(action: unknown, context?: { title?: string; description?: string }): Promise<Receipt>;
}

const WalletContext = createContext<WalletValue | null>(null);
const SESSION_KEY = 'keel-harbor-connected';

/** Stands in for the extension under `VITE_API_MOCK`: connected from the start, signs nothing. */
function demoProvider(network: string): KeelProvider {
  const account: AccountInfo = { address: MOCK_ADDRESS, network, chainId: 0 };
  const refuse = () => Promise.reject(new Error('This is demo data: nothing is signed or sent.'));
  return {
    isKeel: true,
    version: 'demo',
    connect: () => Promise.resolve(account),
    disconnect: () => Promise.resolve(),
    getAccount: () => Promise.resolve(account),
    signMessage: refuse,
    signAction: refuse,
    authorizeSession: refuse,
    on: () => () => {},
  };
}

export function WalletProvider({ children }: { children: ReactNode }) {
  const { network } = useNetwork();
  const rpc = useMemo(() => new RpcClient(rpcUrlFor(network)), [network]);
  const [provider, setProvider] = useState<KeelProvider | null>(null);
  const [account, setAccount] = useState<AccountInfo | null>(null);
  const [detected, setDetected] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const detect = useCallback(() => (isMockEnabled() ? Promise.resolve(demoProvider(network.id)) : detectProvider(1500)), [network.id]);

  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    void detect().then(async (p) => {
      if (cancelled) return;
      setDetected(true);
      setProvider(p);
      if (!p) return;
      const offAccount = p.on('accountChanged', (payload) => setAccount((payload as AccountInfo | null) ?? null));
      const offDisconnect = p.on('disconnect', () => setAccount(null));
      off = () => {
        offAccount();
        offDisconnect();
      };
      // The demo opens connected; Disconnect shows the landing page.
      let wanted = isMockEnabled();
      try {
        wanted ||= window.sessionStorage.getItem(SESSION_KEY) === '1';
      } catch {
        // storage unavailable
      }
      if (wanted) {
        try {
          const a = await p.getAccount();
          if (!cancelled) setAccount(a);
        } catch {
          // the wallet is locked or the site lost its permission; stay disconnected
        }
      }
    });
    return () => {
      cancelled = true;
      off?.();
    };
  }, [detect]);

  const connect = useCallback(async () => {
    setError(null);
    const p = provider ?? (await detect());
    if (!p) {
      setDetected(true);
      setError('Keel Wallet is not installed or not enabled on this site.');
      return;
    }
    setProvider(p);
    setBusy(true);
    try {
      const a = await p.connect({ network: network.id });
      setAccount(a);
      try {
        window.sessionStorage.setItem(SESSION_KEY, '1');
      } catch {
        // best effort
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [provider, detect, network.id]);

  const disconnect = useCallback(async () => {
    try {
      await provider?.disconnect();
    } catch {
      // already gone
    }
    setAccount(null);
    try {
      window.sessionStorage.removeItem(SESSION_KEY);
    } catch {
      // best effort
    }
  }, [provider]);

  const signAndSubmit = useCallback(
    async (action: unknown, context?: { title?: string; description?: string }) => {
      if (!provider || !account) throw new Error('Connect Keel Wallet first.');
      const acct = (await rpc.account(account.address)) as { nonce?: number };
      const nonce = Number(acct.nonce ?? 0);
      const signed = await provider.signAction({ envelope: { signer: account.address, nonce, chain_id: account.chainId, action }, context });
      const tx = await rpc.submit(signed.signed as SignedAction);
      return rpc.waitReceipt(tx);
    },
    [provider, account, rpc],
  );

  const value = useMemo<WalletValue>(
    () => ({ provider, account, detected, busy, error, connect, disconnect, signAndSubmit }),
    [provider, account, detected, busy, error, connect, disconnect, signAndSubmit],
  );
  return <WalletContext.Provider value={value}>{children}</WalletContext.Provider>;
}

export function useWallet(): WalletValue {
  const v = useContext(WalletContext);
  if (!v) throw new Error('useWallet outside WalletProvider');
  return v;
}
