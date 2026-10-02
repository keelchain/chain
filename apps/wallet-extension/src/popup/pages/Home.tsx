import { useEffect, useState } from 'react';
import { call, fetchAccount, type AccountResponse, type UiState } from '../ui';
import type { DepositAddressInfo } from '../../background/wallet';
import { Address } from '../components/Address';
import { SiteAccessBanner } from '../components/SiteAccess';
import { decimalsOf, formatAmount, sentence } from '../../core/format';
import { explorerAccountUrl } from '../../core/networks';

export function Home({ state, onState }: { state: UiState; onState: (s: UiState) => void }) {
  const { network, address } = state;
  const [account, setAccount] = useState<AccountResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [deposit, setDeposit] = useState<DepositAddressInfo | null>(null);
  const [depositBusy, setDepositBusy] = useState<string | null>(null);
  const [depositError, setDepositError] = useState<string | null>(null);
  const [sendKind, setSendKind] = useState<'transfer' | 'withdraw'>('transfer');
  const [sendAsset, setSendAsset] = useState('');
  const [sendTo, setSendTo] = useState('');
  const [sendAmount, setSendAmount] = useState('');
  const [sendMemo, setSendMemo] = useState('');
  const [sendConfirm, setSendConfirm] = useState(false);
  const [sendBusy, setSendBusy] = useState(false);
  const [sendResult, setSendResult] = useState<string | null>(null);

  const doSend = async () => {
    setSendBusy(true);
    try {
      const decimals = decimalsOf(sendAsset);
      const [whole, frac = ''] = sendAmount.split('.');
      if (!/^\d+$/.test(whole ?? '') || !/^\d*$/.test(frac) || frac.length > decimals) throw new Error(`Amount must be a number with at most ${decimals} decimals.`);
      const raw = (BigInt(whole ?? '0') * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals))).toString();
      const r = await call<{ txId: string; ok: boolean; error: string | null }>('send', { kind: sendKind, asset: sendAsset, to: sendTo, amount: raw, memo: sendMemo });
      setSendResult(r.ok ? `done: ${r.txId}${r.error ? ` (${r.error})` : ''}` : `rejected: ${r.error ?? 'unknown'} (${r.txId})`);
      setSendConfirm(false);
      setRefreshKey((k) => k + 1);
    } catch (e) {
      setSendResult(e instanceof Error ? e.message : String(e));
    } finally {
      setSendBusy(false);
    }
  };

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
      (a) => {
        if (!live) return;
        setAccount(a);
        const first = a.balances.find((b) => b.account_type === 'deposit')?.asset;
        if (first) setSendAsset((cur) => cur || first);
      },
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
          <div className="label">Balances</div>
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
                  <td className="muted">{sentence(b.account_type)}</td>
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
          <p className="muted small">Fund this account from another chain. The address belongs to the Keelchain vault; the chain credits you after {network.id === 'testnet' ? 'the confirmations for that chain' : 'confirmation'}.</p>
          <div className="row">
            {['BTC', 'TRON'].map((c) => (
              <button key={c} className="mini" disabled={depositBusy !== null} onClick={() => receive(c)}>
                {depositBusy === c ? 'Requesting…' : c === 'BTC' ? 'Bitcoin' : 'Tron (USDT, TRX)'}
              </button>
            ))}
          </div>
          {depositError && <p className="error">{depositError}</p>}
          {deposit && (
            <div>
              <div className="label">{deposit.label} deposit address · index {deposit.index}{deposit.fresh ? ' · just assigned' : ''}</div>
              <Address value={deposit.address} full />
              <p className="muted small">{network.id === 'testnet' ? (deposit.chain === 'BTC' ? 'Signet coins only (a signet faucet works). Credited as BTC.BTC after 2 confirmations.' : 'Nile testnet only. USDT is credited as TRON.USDT, TRX as TRON.TRX, after 19 confirmations.') : 'Send only the assets of this chain to this address.'}</p>
            </div>
          )}
        </div>
      )}
      {!network.placeholder && address && (
        <div className="card">
          <div className="label">Send</div>
          <div className="row">
            <select value={sendKind} onChange={(e) => setSendKind(e.target.value as 'transfer' | 'withdraw')} title="Kind">
              <option value="transfer">Transfer on Keel</option>
              <option value="withdraw">Withdraw to another chain</option>
            </select>
            <select value={sendAsset} onChange={(e) => setSendAsset(e.target.value)} title="Asset">
              {(account?.balances ?? []).filter((b) => b.account_type === 'deposit').map((b) => <option key={b.asset} value={b.asset}>{b.asset}</option>)}
            </select>
          </div>
          <input value={sendTo} placeholder={sendKind === 'transfer' ? 'Recipient Keel address (64 hex)' : 'Destination address on that chain'} onChange={(e) => setSendTo(e.target.value.trim())} />
          <div className="row">
            <input value={sendAmount} placeholder={`Amount in ${sendAsset || 'units'}`} onChange={(e) => setSendAmount(e.target.value.trim())} />
            {sendKind === 'transfer' && <input value={sendMemo} placeholder="Memo (optional)" onChange={(e) => setSendMemo(e.target.value)} />}
          </div>
          {sendConfirm ? (
            <div>
              <p className="small">{sendKind === 'transfer' ? 'Transfer' : 'Withdraw'} <b>{sendAmount} {sendAsset}</b> to <code className="small">{sendTo}</code>{sendKind === 'withdraw' ? ' (network fee is taken from the amount)' : ''}?</p>
              <div className="row">
                <button disabled={sendBusy} onClick={() => void doSend()}>{sendBusy ? 'Signing…' : 'Confirm'}</button>
                <button className="mini" disabled={sendBusy} onClick={() => setSendConfirm(false)}>Back</button>
              </div>
            </div>
          ) : (
            <button disabled={!sendAsset || !sendTo || !sendAmount} onClick={() => { setSendResult(null); setSendConfirm(true); }}>Review</button>
          )}
          {sendResult && <p className={sendResult.startsWith('done') ? 'small' : 'error'}>{sendResult}</p>}
        </div>
      )}
      <div className="row">
        <button onClick={() => void act('addAccount')}>Add account</button>
        <span className="muted small">auto-lock in {Math.ceil(state.remainingMs / 60_000)} min</span>
      </div>
    </div>
  );
}
