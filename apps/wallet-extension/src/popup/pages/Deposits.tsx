/**
 * Deposits in flight and landed for the active account: the chain's own rows
 * (observers voting, credited) merged with what the external networks show
 * arriving at the deposit addresses, looked up on their public APIs from the
 * popup. Refreshes every 30 seconds while the tab is open.
 */
import { useEffect, useState } from 'react';
import { depositProgress, externalNetworks, externalTxUrl, fetchIncoming, isProcessing, stageLabel, type DepositProgress, type ExternalChain } from '@keelchain/sdk';
import { fetchChainDeposits, fetchDepositAddress, fetchParams, type UiState } from '../ui';
import { decimalsOf, formatAmount } from '../../core/format';
import { DEPOSIT_CHAINS } from '../external';

const REQUIRED_PARAM: Record<ExternalChain, string> = { BTC: 'confirmations_btc', ETH: 'confirmations_eth', TRON: 'confirmations_tron' };
const REQUIRED_DEFAULT: Record<ExternalChain, number> = { BTC: 2, ETH: 12, TRON: 19 };

export function Deposits({ state }: { state: UiState }) {
  const { network, address } = state;
  const rpc = network.rpc;
  const networkId = network.id;
  const [rows, setRows] = useState<DepositProgress[] | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => setTick((n) => n + 1), 30_000);
    return () => clearInterval(t);
  }, []);
  useEffect(() => {
    if (!address || network.placeholder) return;
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
  }, [rpc, networkId, network.placeholder, address, tick]);
  const processing = (rows ?? []).filter((p) => isProcessing(p.stage)).length;
  const nets = externalNetworks(networkId === 'mainnet' ? 'mainnet' : 'testnet');
  if (!address || network.placeholder) return <div className="page"><p className="muted">{network.placeholder ? `${network.name}: no RPC configured yet.` : 'No account.'}</p></div>;
  return (
    <div className="page">
      <div className="card">
        <div className="row">
          <div className="label">Deposits{processing > 0 && <> <span className="badge warn">{processing} processing</span></>}</div>
          <button className="mini" onClick={() => setTick((n) => n + 1)}>Refresh</button>
        </div>
        <p className="muted small">Every deposit to your Keel addresses, from the moment {DEPOSIT_CHAINS.map((c) => nets[c].name).join(' or ')} sees it, through its confirmations and the observers' votes, until the chain credits it.</p>
        {rows === null ? (
          <p className="muted small">Looking for deposits…</p>
        ) : rows.length === 0 ? (
          <p className="muted small">Nothing incoming and nothing credited yet.</p>
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
    </div>
  );
}
