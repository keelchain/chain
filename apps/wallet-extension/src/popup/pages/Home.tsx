import { useEffect, useState } from 'react';
import { call, fetchAccount, type AccountResponse, type UiState } from '../ui';
import { Address } from '../components/Address';
import { decimalsOf, formatAmount, sentence } from '../../core/format';
import { explorerAccountUrl } from '../../core/networks';

export function Home({ state, onState }: { state: UiState; onState: (s: UiState) => void }) {
  const { network, address } = state;
  const [account, setAccount] = useState<AccountResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    if (!address || !network.rpc) {
      setAccount(null);
      return;
    }
    let live = true;
    setError(null);
    fetchAccount(network.rpc, address).then(
      (a) => live && setAccount(a),
      (e: unknown) => live && setError(`Cannot reach ${network.rpc}: ${e instanceof Error ? e.message : String(e)}`),
    );
    return () => {
      live = false;
    };
  }, [address, network.rpc, network.id, refreshKey]);

  const act = (method: string, params?: unknown) => call<UiState>(method, params).then(onState, (e: unknown) => setError(String(e)));

  return (
    <div className="page">
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
      <div className="row">
        <button onClick={() => void act('addAccount')}>Add account</button>
        <span className="muted small">auto-lock in {Math.ceil(state.remainingMs / 60_000)} min</span>
      </div>
    </div>
  );
}
