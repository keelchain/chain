import { useEffect, useState } from 'react';
import { depositProgress, externalNetworks, externalTxUrl, fetchIncoming, isProcessing, stageLabel, type DepositProgress, type ExternalChain } from '@keelchain/sdk';
import { call, fetchAccount, fetchChainDeposits, fetchDepositAddress, fetchParams, type AccountResponse, type UiState } from '../ui';
import type { DepositAddressInfo } from '../../background/wallet';
import { Address } from '../components/Address';
import { SiteAccessBanner } from '../components/SiteAccess';
import { decimalsOf, formatAmount, sentence } from '../../core/format';
import { explorerAccountUrl } from '../../core/networks';

const DEPOSIT_CHAINS: ExternalChain[] = ['BTC', 'TRON'];
const REQUIRED_PARAM: Record<ExternalChain, string> = { BTC: 'confirmations_btc', ETH: 'confirmations_eth', TRON: 'confirmations_tron' };
const REQUIRED_DEFAULT: Record<ExternalChain, number> = { BTC: 2, ETH: 12, TRON: 19 };

/** Name of the external network an id maps to: the testnet bridges signet and Nile. */
function externalName(networkId: string, chain: ExternalChain): string {
  return externalNetworks(networkId === 'mainnet' ? 'mainnet' : 'testnet')[chain].name;
}

/**
 * Deposits in flight and landed for the active account: the chain's own rows
 * (observers voting, credited) merged with what the external networks show
 * arriving at the deposit addresses, looked up on their public APIs from the
 * popup. Refreshes every 30 seconds while the popup is open.
 */
function Deposits({ rpc, networkId, address, refreshKey }: { rpc: string; networkId: string; address: string; refreshKey: number }) {
  const [rows, setRows] = useState<DepositProgress[] | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(t);
  }, []);
  useEffect(() => {
    let cancelled = false;
    const nets = externalNetworks(networkId === 'mainnet' ? 'mainnet' : 'testnet');
    (async () => {
      const [chainRows, params, addresses] = await Promise.all([
        fetchChainDeposits(rpc, address).catch(() => []),
        fetchParams(rpc).catch(() => ({}) as Record<string, unknown>),
        Promise.all(DEPOSIT_CHAINS.map((c) => fetchDepositAddress(rpc, address, c).catch(() => null))),
      ]);
      const failed: string[] = [];
      const all: DepositProgress[] = [];
      for (const [i, chain] of DEPOSIT_CHAINS.entries()) {
        const dep = addresses[i];
        const net = nets[chain];
        const required = Number(params[REQUIRED_PARAM[chain]] ?? 0) || REQUIRED_DEFAULT[chain];
        let incoming = null;
        if (dep && net.api) {
          try {
            incoming = await fetchIncoming(net, dep.address);
          } catch {
            failed.push(net.name);
          }
        }
        all.push(...depositProgress(incoming, chainRows.filter((r) => r.chain === chain), required));
      }
      if (!cancelled) {
        setRows(all);
        setUnreachable(failed);
      }
    })();
    return () => { cancelled = true; };
  }, [rpc, networkId, address, refreshKey, tick]);
  const processing = (rows ?? []).filter((p) => isProcessing(p.stage)).length;
  const nets = externalNetworks(networkId === 'mainnet' ? 'mainnet' : 'testnet');
  return (
    <div className="card">
      <div className="row">
        <div className="label">Deposits{processing > 0 && <> <span className="badge warn">{processing} processing</span></>}</div>
        <button className="mini" onClick={() => setTick((n) => n + 1)}>Refresh</button>
      </div>
      {rows === null ? (
        <p className="muted small">Looking for deposits…</p>
      ) : rows.length === 0 ? (
        <p className="muted small">Nothing incoming. A deposit shows here from the moment {DEPOSIT_CHAINS.map((c) => nets[c].name).join(' or ')} sees it, through its confirmations, until the chain credits it.</p>
      ) : (
        <ul className="list">
          {rows.map((p) => (
            <li key={`${p.chain}:${p.tx_hash}`}>
              <span className="small">
                {nets[p.chain as ExternalChain] ? (
                  <a href={externalTxUrl(nets[p.chain as ExternalChain], p.tx_hash)} target="_blank" rel="noreferrer" title={p.tx_hash}>{formatAmount(p.amount, decimalsOf(p.asset))} {p.asset}</a>
                ) : (
                  <span title={p.tx_hash}>{formatAmount(p.amount, decimalsOf(p.asset))} {p.asset}</span>
                )}
                <br />
                <span className="muted">{stageLabel(p)}</span>
              </span>
              <span className={`badge ${p.stage === 'credited' ? 'good' : p.stage === 'rejected' ? 'bad' : 'warn'}`}>{p.stage}</span>
            </li>
          ))}
        </ul>
      )}
      {unreachable.length > 0 && <p className="muted small">Could not reach {unreachable.join(' and ')} to look for incoming transactions; the chain's own records are shown.</p>}
    </div>
  );
}

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
          <p className="muted small">Fund this account from {externalName(network.id, 'BTC')} or {externalName(network.id, 'TRON')}. The address belongs to the Keelchain vault; the chain credits you after that chain's confirmations. Coins sent on any other network do not arrive.</p>
          <div className="row">
            {DEPOSIT_CHAINS.map((c) => (
              <button key={c} className="mini" disabled={depositBusy !== null} onClick={() => receive(c)}>
                {depositBusy === c ? 'Requesting…' : c === 'BTC' ? externalName(network.id, 'BTC') : `${externalName(network.id, 'TRON')} (USDT, TRX)`}
              </button>
            ))}
          </div>
          {depositError && <p className="error">{depositError}</p>}
          {deposit && (
            <div>
              <div className="label">{externalName(network.id, deposit.chain as ExternalChain)} deposit address · index {deposit.index}{deposit.fresh ? ' · just assigned' : ''}</div>
              <Address value={deposit.address} full />
              <p className="notice small">{network.id === 'testnet' ? (deposit.chain === 'BTC' ? 'Bitcoin signet only: coins sent on Bitcoin mainnet, testnet3 or testnet4 never arrive. A signet faucet works. Credited as BTC.BTC after 2 confirmations.' : 'Tron Nile only: USDT or TRX sent on Tron mainnet or Shasta never arrive. USDT is credited as TRON.USDT, TRX as TRON.TRX, after 19 confirmations.') : 'Send only the assets of this chain, on this chain, to this address.'}</p>
            </div>
          )}
        </div>
      )}
      {!network.placeholder && address && <Deposits rpc={network.rpc} networkId={network.id} address={address} refreshKey={refreshKey} />}
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
