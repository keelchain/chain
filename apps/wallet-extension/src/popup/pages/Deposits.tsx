/**
 * Deposits in flight and landed for the active account: the chain's own rows
 * (observers voting, credited) merged with what the external networks show
 * arriving at the deposit addresses, looked up on their public APIs from the
 * popup. Refreshes every 30 seconds while the tab is open.
 */
import { useEffect, useRef, useState } from 'react';
import { depositProgress, externalNetworks, externalTxUrl, fetchIncoming, isProcessing, stageLabel, type DepositProgress, type ExternalChain } from '@keelchain/sdk';
import { call, fetchChainDeposits, fetchDepositAddress, fetchParams, type UiState } from '../ui';
import { decimalsOf, formatAmount } from '../../core/format';
import { DEPOSIT_CHAINS, externalName } from '../external';
import type { RecoveryRow } from '../../background/wallet';

// BSC shares the EVM depth with Ethereum (`confirmations_eth` on the chain).
const REQUIRED_PARAM: Record<ExternalChain, string> = { BTC: 'confirmations_btc', ETH: 'confirmations_eth', TRON: 'confirmations_tron', BSC: 'confirmations_eth', SOL: 'confirmations_tron' };
const REQUIRED_DEFAULT: Record<ExternalChain, number> = { BTC: 2, ETH: 12, TRON: 19, BSC: 12, SOL: 19 };

export function Deposits({ state }: { state: UiState }) {
  const { network, address } = state;
  const rpc = network.rpc;
  const networkId = network.id;
  const [rows, setRows] = useState<DepositProgress[] | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [tick, setTick] = useState(0);
  // What the last refreshes showed. The public APIs are eventually consistent
  // and sometimes refuse a call, and the chain only lists a deposit once an
  // observer reported it, so a transaction can be missing from one refresh
  // and back in the next. A deposit still being processed stays on screen at
  // its last known stage until it reappears, for up to ten minutes.
  const memory = useRef(new Map<string, { p: DepositProgress; seen: number }>());
  useEffect(() => {
    memory.current.clear();
  }, [address, networkId]);
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
      const nowMs = Date.now();
      const present = new Set(all.map((p) => `${p.chain}:${p.tx_hash}`));
      for (const p of all) memory.current.set(`${p.chain}:${p.tx_hash}`, { p, seen: nowMs });
      for (const [k, v] of memory.current) {
        if (present.has(k)) continue;
        if (isProcessing(v.p.stage) && nowMs - v.seen < 10 * 60_000) all.push(v.p);
        else memory.current.delete(k);
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
  const [recoveries, setRecoveries] = useState<RecoveryRow[]>([]);
  const [recChain, setRecChain] = useState<string>('BSC');
  const [recEvmId, setRecEvmId] = useState('');
  const [recToken, setRecToken] = useState('');
  const [recTo, setRecTo] = useState('');
  const [recBusy, setRecBusy] = useState(false);
  const [recMsg, setRecMsg] = useState<string | null>(null);
  useEffect(() => {
    if (!address || network.placeholder) return;
    call<RecoveryRow[]>('recoveries').then(setRecoveries, () => setRecoveries([]));
  }, [address, networkId, tick, recBusy, network.placeholder]);
  const recover = async () => {
    setRecBusy(true);
    setRecMsg(null);
    try {
      const r = await call<{ txId: string; ok: boolean; error: string | null }>('requestRecovery', { chain: recChain, evmChainId: recEvmId, token: recToken, to: recTo });
      setRecMsg(r.ok ? 'Requested. The observers move the coins once they see them; a token on an unserved network needs a little native coin at the address for gas.' : `Refused: ${r.error}`);
      if (r.ok) { setRecTo(''); setRecToken(''); setRecEvmId(''); }
    } catch (e) {
      setRecMsg(e instanceof Error ? e.message : String(e));
    } finally {
      setRecBusy(false);
    }
  };
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
                    <a href={externalTxUrl(nets[p.chain as ExternalChain], p.external_tx ?? p.tx_hash)} target="_blank" rel="noreferrer" title={p.external_tx ?? p.tx_hash}>{formatAmount(p.amount, decimalsOf(p.asset))} {p.asset}</a>
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
      <div className="card">
        <div className="label">Sent on the wrong network?</div>
        <p className="muted small">Coins sent to one of your Keel addresses on a network the chain does not serve, or in a token it does not list, are not credited, but they are yours: ask the observers to move them to an address of your own there. The chain checks the address is yours; nobody approves anything. A token transfer on an unserved EVM network needs a little of that network's native coin at the same address for gas.</p>
        <label className="small">Address family
          <select value={recChain} onChange={(e) => setRecChain(e.target.value)}>
            {DEPOSIT_CHAINS.filter((c) => c !== 'BTC').map((c) => <option key={c} value={c}>{externalName(network.id, c)} address</option>)}
          </select>
        </label>
        {(recChain === 'BSC' || recChain === 'ETH') && (
          <label className="small">EVM chain id, when the coins are on another EVM network
            <input value={recEvmId} onChange={(e) => setRecEvmId(e.target.value)} placeholder="137 Polygon, 42161 Arbitrum, 1 Ethereum" />
          </label>
        )}
        <label className="small">Token contract or mint (empty for the native coin)
          <input value={recToken} onChange={(e) => setRecToken(e.target.value)} placeholder="0x… / base58 mint / T…" />
        </label>
        <label className="small">Send them to
          <input value={recTo} onChange={(e) => setRecTo(e.target.value)} placeholder="your address on that network" />
        </label>
        <button className="mini" disabled={recBusy || recTo.trim().length < 20} onClick={() => void recover()}>{recBusy ? 'Requesting…' : 'Recover'}</button>
        {recMsg && <p className="small">{recMsg}</p>}
        {recoveries.length > 0 && (
          <ul className="list">
            {recoveries.map((r) => (
              <li key={r.id}>
                <span className="small">
                  #{r.id} {r.token ? `token ${r.token.slice(0, 10)}…` : 'native coin'} on {r.evm_chain_id ? `chain id ${r.evm_chain_id}` : externalName(network.id, r.chain as ExternalChain)} → {r.to.slice(0, 10)}…
                  {r.status === 'Done' && <><br /><span className="muted">moved {r.amount} units{r.external_tx ? ` · ${r.external_tx.slice(0, 12)}…` : r.tx_hash ? ` · ${r.tx_hash.slice(0, 12)}…` : ''}</span></>}
                </span>
                <span className={`badge ${r.status === 'Done' ? 'good' : r.status === 'Open' ? 'warn' : 'bad'}`}>{r.status}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
