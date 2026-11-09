/**
 * "In your wallet": the active account's own Bitcoin and Tron addresses, what
 * they hold, and the button that moves coins from them into Keel. The
 * balances come from the public chain APIs; the move is built, signed and
 * broadcast by the background with the key derived from the seed.
 */
import { useEffect, useState } from 'react';
import { externalNetworks, externalTxUrl, type ExternalChain } from '@keelchain/sdk';
import type { SelfAddresses, SelfBalances } from '../../background/wallet';
import { decimalsOf, formatAmount } from '../../core/format';
import { call } from '../ui';
import { Address } from './Address';

export function chainOfAsset(asset: string): ExternalChain | null {
  const c = asset.split('.')[0];
  return c === 'BTC' || c === 'TRON' || c === 'ETH' ? c : null;
}

export function SelfCustody({ networkId, address, refreshKey, onMoved }: { networkId: string; address: string; refreshKey: number; onMoved: () => void }) {
  const [addrs, setAddrs] = useState<SelfAddresses | null>(null);
  const [bal, setBal] = useState<SelfBalances | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [asset, setAsset] = useState('');
  const [amount, setAmount] = useState('');
  const [max, setMax] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ txid: string; asset: string; amount: string } | null>(null);
  const [tick, setTick] = useState(0);
  const nets = externalNetworks(networkId === 'mainnet' ? 'mainnet' : 'testnet');

  useEffect(() => {
    let cancelled = false;
    setError(null);
    call<SelfAddresses>('selfAddresses').then((a) => { if (!cancelled) setAddrs(a); }, (e: unknown) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); });
    call<SelfBalances>('selfBalances').then((b) => { if (!cancelled) setBal(b); }, (e: unknown) => { if (!cancelled) setError(e instanceof Error ? e.message : String(e)); });
    return () => { cancelled = true; };
  }, [networkId, address, refreshKey, tick]);

  if (addrs && !addrs.supported) return null;
  const held = (bal?.balances ?? []).filter((b) => BigInt(b.amount) > 0n);
  const chosen = asset || held[0]?.asset || '';
  const chosenBal = bal?.balances.find((b) => b.asset === chosen);
  const decimals = chosen ? decimalsOf(chosen) : 0;

  const toUnits = (text: string): string | null => {
    const [whole, frac = ''] = text.trim().split('.');
    if (!/^\d+$/.test(whole ?? '') || !/^\d*$/.test(frac) || frac.length > decimals) return null;
    return (BigInt(whole ?? '0') * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals))).toString();
  };

  const move = async () => {
    setBusy(true);
    setError(null);
    try {
      const chain = chainOfAsset(chosen);
      if (!chain) throw new Error('Choose an asset.');
      const units = max ? '0' : toUnits(amount);
      if (units === null) throw new Error(`Amount must be a number with at most ${decimals} decimals.`);
      const r = await call<{ txid: string; amount: string; fee: string; asset: string }>('moveToKeel', { chain, asset: chosen, amount: units, max });
      setResult({ txid: r.txid, asset: r.asset, amount: r.amount });
      setConfirm(false);
      setAmount('');
      setMax(false);
      onMoved();
      setTick((n) => n + 1);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setConfirm(false);
    } finally {
      setBusy(false);
    }
  };

  const chainName = (c: string) => nets[c as ExternalChain]?.name ?? c;

  return (
    <div className="card">
      <div className="row">
        <div className="label">In your wallet</div>
        <button className="mini" onClick={() => setTick((n) => n + 1)}>Refresh</button>
      </div>
      <p className="muted small">Coins under your own key, outside Keel. Only your seed phrase can move them; no observer set stands behind them. Move them to Keel to trade.</p>
      {!addrs || !bal ? (
        <p className="muted small">Loading…</p>
      ) : (
        <>
          <table className="balances">
            <tbody>
              {bal.balances.map((b) => (
                <tr key={b.asset}>
                  <td>{b.asset}</td>
                  <td className="muted">{chainName(b.chain)}{BigInt(b.pending) > 0n ? ` · ${formatAmount(b.pending, decimalsOf(b.asset))} unconfirmed` : ''}</td>
                  <td className="num">{formatAmount(b.amount, decimalsOf(b.asset))}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {bal.unreachable.length > 0 && <p className="muted small">Could not reach {bal.unreachable.join(' and ')}; balances there are not shown.</p>}
          {(['BTC', 'TRON'] as const).map((c) => {
            const a = addrs.addresses[c];
            if (!a) return null;
            return (
              <div key={c}>
                <div className="label">{chainName(c)} · your address <span className="muted">({a.path})</span></div>
                <Address value={a.address} full />
              </div>
            );
          })}
          {held.length > 0 && (
            <div>
              <div className="label">Move to Keel</div>
              <div className="row">
                <select value={chosen} onChange={(e) => { setAsset(e.target.value); setMax(false); }} title="Asset">
                  {held.map((b) => <option key={b.asset} value={b.asset}>{b.asset}</option>)}
                </select>
                <input value={max ? (chosenBal ? formatAmount(chosenBal.amount, decimals) : '') : amount} disabled={max} placeholder={`Amount in ${chosen}`} onChange={(e) => setAmount(e.target.value.trim())} />
                <label className="small"><input type="checkbox" checked={max} onChange={(e) => setMax(e.target.checked)} /> Max</label>
              </div>
              {confirm ? (
                <div>
                  <p className="small">
                    Move <b>{max ? 'everything' : `${amount} ${chosen}`}</b> from your {chainName(chainOfAsset(chosen) ?? '')} address to your Keel deposit address? The {chainName(chainOfAsset(chosen) ?? '')} network fee is paid from your wallet{chosen === 'BTC.BTC' && max ? ' and taken from the amount' : ''}. It lands in Keel after the usual confirmations.
                  </p>
                  <div className="row">
                    <button disabled={busy} onClick={() => void move()}>{busy ? 'Broadcasting…' : 'Confirm'}</button>
                    <button className="mini" disabled={busy} onClick={() => setConfirm(false)}>Back</button>
                  </div>
                </div>
              ) : (
                <button disabled={!chosen || (!max && !amount)} onClick={() => { setResult(null); setConfirm(true); }}>Review</button>
              )}
            </div>
          )}
          {result && (
            <p className="small">
              Sent {formatAmount(result.amount, decimalsOf(result.asset))} {result.asset} to Keel:{' '}
              {nets[chainOfAsset(result.asset) ?? 'BTC'] ? <a href={externalTxUrl(nets[chainOfAsset(result.asset) ?? 'BTC'], result.txid)} target="_blank" rel="noreferrer">{result.txid.slice(0, 12)}…</a> : result.txid.slice(0, 12)}. The Deposits card follows it from here.
            </p>
          )}
        </>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  );
}
