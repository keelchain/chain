import { useEffect, useState } from 'react';
import type { ExternalChain } from '@keelchain/sdk';
import { call, fetchAccount, type AccountResponse, type UiState } from '../ui';
import type { DepositAddressInfo, SelfAddresses } from '../../background/wallet';
import { Address } from '../components/Address';
import { SiteAccessBanner } from '../components/SiteAccess';
import { accountTypeLabel, decimalsOf, formatAmount } from '../../core/format';
import { explorerAccountUrl } from '../../core/networks';
import { DEPOSIT_CHAINS, externalName } from '../external';

/** The account: network and account selectors, address, balances in Keel, and where to receive. */
export function Home({ state, onState }: { state: UiState; onState: (s: UiState) => void }) {
  const { network, address } = state;
  const [account, setAccount] = useState<AccountResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [deposit, setDeposit] = useState<DepositAddressInfo | null>(null);
  const [ownAddrs, setOwnAddrs] = useState<SelfAddresses | null>(null);
  const toWallet = state.settings.depositsTo === 'wallet';
  const [showOther, setShowOther] = useState(false);
  // Which side the Receive card shows: the setting, flipped by the "show the other" link.
  const receiveOwn = toWallet !== showOther;
  useEffect(() => {
    setOwnAddrs(null);
    if (!address || network.placeholder) return;
    call<SelfAddresses>('selfAddresses').then(setOwnAddrs, () => setOwnAddrs(null));
  }, [address, network.id, network.placeholder, refreshKey]);
  const [depositBusy, setDepositBusy] = useState<string | null>(null);
  const [depositError, setDepositError] = useState<string | null>(null);

  const receive = (chain: string) => {
    setDepositBusy(chain);
    setDepositError(null);
    call<DepositAddressInfo>('depositAddress', { chain }).then(
      (d) => { setDeposit(d); setDepositBusy(null); },
      (e: unknown) => { setDepositError(e instanceof Error ? e.message : String(e)); setDepositBusy(null); },
    );
  };

  useEffect(() => {
    setDeposit(null);
    setDepositError(null);
    if (!address || !network.rpc) {
      setAccount(null);
      return;
    }
    let live = true;
    setError(null);
    fetchAccount(network.rpc, address).then(
      (a) => { if (live) setAccount(a); },
      (e: unknown) => live && setError(`Cannot reach ${network.rpc}: ${e instanceof Error ? e.message : String(e)}`),
    );
    return () => {
      live = false;
    };
  }, [address, network.rpc, network.id, refreshKey]);

  const act = (method: string, params?: unknown) => call<UiState>(method, params).then(onState, (e: unknown) => setError(String(e)));

  return (
    <div className="page">
      <SiteAccessBanner />
      <div className="row">
        <select value={network.id} onChange={(e) => void act('switchNetwork', { id: e.target.value })} title="Network">
          {state.networks.map((n) => <option key={n.id} value={n.id}>{n.name}</option>)}
        </select>
        <select value={state.activeIndex} onChange={(e) => void act('selectAccount', { index: Number(e.target.value) })} title="Account">
          {state.accounts.map((a) => <option key={a.index} value={a.index}>{a.name}</option>)}
        </select>
      </div>
      {address && (
        <div className="card">
          <div className="label">Address</div>
          <Address value={address} full />
          <div className="row small">
            {explorerAccountUrl(network, address) && <a href={explorerAccountUrl(network, address)} target="_blank" rel="noreferrer">View in explorer ↗</a>}
            <span className="muted">chain id {network.chainId}</span>
          </div>
        </div>
      )}
      <div className="card">
        <div className="row">
          <div className="label">In Keel</div>
          <button className="mini" onClick={() => setRefreshKey((k) => k + 1)}>Refresh</button>
        </div>
        {network.placeholder ? (
          <p className="muted">{network.name}: no RPC configured yet.</p>
        ) : error ? (
          <p className="error">{error}</p>
        ) : !account ? (
          <p className="muted">Loading…</p>
        ) : account.balances.length === 0 ? (
          <p className="muted">No balances yet.</p>
        ) : (
          <table className="balances">
            <tbody>
              {account.balances.map((b, i) => (
                <tr key={i}>
                  <td>{b.asset}</td>
                  <td className="muted">{accountTypeLabel(b.account_type)}</td>
                  <td className="num">{formatAmount(b.balance, decimalsOf(b.asset))}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {account && <p className="muted small">nonce {account.nonce}{account.tier !== undefined ? ` · tier ${account.tier}` : ''}</p>}
      </div>
      {!network.placeholder && address && (
        <div className="card">
          <div className="label">Receive</div>
          {receiveOwn && ownAddrs?.supported ? (
            <>
              <p className="muted small">New deposits go to <b>your wallet</b>: addresses only your seed phrase controls, outside Keel. Move coins to Keel from the Wallet tab when you want to trade. <a href="#" onClick={(e) => { e.preventDefault(); setShowOther((v) => !v); }}>Show the Keel address instead</a></p>
              {DEPOSIT_CHAINS.map((c) => {
                const a = ownAddrs.addresses[c as 'BTC' | 'TRON'];
                return a ? (
                  <div key={c}>
                    <div className="label">{externalName(network.id, c)} · your address</div>
                    <Address value={a.address} full />
                  </div>
                ) : null;
              })}
            </>
          ) : (
            <>
              <p className="muted small">Fund this account from {externalName(network.id, 'BTC')} or {externalName(network.id, 'TRON')}. The address belongs to the Keelchain vault; the chain credits you after that chain's confirmations. Coins sent on any other network do not arrive.{ownAddrs?.supported && <> <a href="#" onClick={(e) => { e.preventDefault(); setShowOther((v) => !v); }}>Show my own address instead</a></>}</p>
              <div className="row">
                {DEPOSIT_CHAINS.map((c) => (
                  <button key={c} className="mini" disabled={depositBusy !== null} onClick={() => receive(c)}>
                    {depositBusy === c ? 'Requesting…' : c === 'BTC' ? externalName(network.id, 'BTC') : `${externalName(network.id, 'TRON')} (USDT, TRX)`}
                  </button>
                ))}
              </div>
            </>
          )}
          {depositError && <p className="error">{depositError}</p>}
          {!receiveOwn && deposit && (
            <div>
              <div className="label">{externalName(network.id, deposit.chain as ExternalChain)} deposit address · index {deposit.index}{deposit.fresh ? ' · just assigned' : ''}</div>
              <Address value={deposit.address} full />
              <p className="notice small">{network.id === 'testnet' ? (deposit.chain === 'BTC' ? 'Bitcoin signet only: coins sent on Bitcoin mainnet, testnet3 or testnet4 never arrive. A signet faucet works. Credited as BTC.BTC after 2 confirmations.' : 'Tron Nile only: USDT or TRX sent on Tron mainnet or Shasta never arrive. USDT is credited as TRON.USDT, TRX as TRON.TRX, after 19 confirmations.') : 'Send only the assets of this chain, on this chain, to this address.'}</p>
            </div>
          )}
        </div>
      )}
      <div className="row">
        <button onClick={() => void act('addAccount')}>Add account</button>
        <span className="muted small">auto-lock in {Math.ceil(state.remainingMs / 60_000)} min</span>
      </div>
    </div>
  );
}
