/**
 * Send from the active account: a transfer on Keel, a withdrawal to any
 * address on the asset's chain, or a move to the user's own wallet address
 * (a withdrawal whose destination is pre-filled and locked). A balance held
 * in an exchange's custody (docs/models.md, Model A) withdraws through the
 * exchange's own vault, and moves between custody and the chain's market
 * with the float swap, no coins moving.
 */
import { useEffect, useState } from 'react';
import { call, fetchAccount, type AccountResponse, type UiState } from '../ui';
import type { SelfAddresses } from '../../background/wallet';
import { decimalsOf, formatAmount } from '../../core/format';
import { BRIDGED_ASSETS, externalName, toUnits, DOLLAR_RAILS } from '../external';
import { Address } from '../components/Address';
import { explorerTxUrl } from '../../core/networks';

type Kind = 'transfer' | 'withdraw' | 'wallet' | 'swap';
type Pool = 'deposit' | 'custody';

export function Send({ state }: { state: UiState }) {
  const { network, address } = state;
  const [account, setAccount] = useState<AccountResponse | null>(null);
  const [ownAddrs, setOwnAddrs] = useState<SelfAddresses | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [kind, setKind] = useState<Kind>('transfer');
  const [asset, setAsset] = useState('');
  const [pool, setPool] = useState<Pool>('deposit');
  const [custodian, setCustodian] = useState<string | null>(null);
  const [to, setTo] = useState('');
  const [via, setVia] = useState(DOLLAR_RAILS[0]!.via);
  const [amount, setAmount] = useState('');
  const [max, setMax] = useState(false);
  const [memo, setMemo] = useState('');
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ txId: string | null; ok: boolean; text: string } | null>(null);

  useEffect(() => {
    if (!address || network.placeholder) return;
    let live = true;
    setError(null);
    fetchAccount(network.rpc, address).then((a) => { if (live) setAccount(a); }, (e: unknown) => live && setError(`Cannot reach ${network.rpc}: ${e instanceof Error ? e.message : String(e)}`));
    call<SelfAddresses>('selfAddresses').then((a) => { if (live) setOwnAddrs(a); }, () => undefined);
    call<{ custodian: string } | null>('custodian').then((c) => { if (live) setCustodian(c?.custodian ?? null); }, () => undefined);
    return () => { live = false; };
  }, [address, network.rpc, network.id, network.placeholder, refreshKey]);

  if (!address || network.placeholder) return <div className="page"><p className="muted">{network.placeholder ? `${network.name}: no RPC configured yet.` : 'No account.'}</p></div>;

  const spendable = new Map<string, string>();
  const inCustody = new Map<string, string>();
  for (const b of account?.balances ?? []) {
    if (b.account_type === 'deposit') spendable.set(b.asset, String(b.balance));
    if (b.account_type === 'custody' && BigInt(String(b.balance)) > 0n) inCustody.set(b.asset, String(b.balance));
  }
  const hasCustody = inCustody.size > 0 || custodian !== null;
  const from: Pool = kind === 'transfer' ? 'deposit' : pool;
  // Transfers: everything the account holds on the market side. Withdrawals and
  // moves: the bridged assets, always listed, from the chosen pool. The swap:
  // whatever the chosen pool holds.
  const options: { asset: string; balance: string }[] =
    kind === 'transfer'
      ? [...spendable.entries()].map(([a, balance]) => ({ asset: a, balance }))
      : kind === 'swap'
        ? [...(from === 'custody' ? inCustody : spendable).entries()].filter(([a]) => a.includes('.')).map(([a, balance]) => ({ asset: a, balance }))
        : [
            // The unified USDT leaves as the chosen network's USDT (one-step redeem and withdraw).
            ...(kind === 'withdraw' && from !== 'custody' ? [{ asset: 'USDT', balance: spendable.get('USDT') ?? '0' }] : []),
            ...BRIDGED_ASSETS.map((b) => ({ asset: b.asset, balance: (from === 'custody' ? inCustody : spendable).get(b.asset) ?? '0' })),
          ];
  const chosen = options.some((o) => o.asset === asset) ? asset : (options[0]?.asset ?? '');
  const balance = options.find((o) => o.asset === chosen)?.balance ?? '0';
  const decimals = chosen ? decimalsOf(chosen) : 0;
  const rail = chosen === 'USDT' ? DOLLAR_RAILS.find((r) => r.via === via) : undefined;
  const chain = rail?.chain ?? BRIDGED_ASSETS.find((b) => b.asset === chosen)?.chain;
  const own = kind === 'wallet' && (chain === 'BTC' || chain === 'TRON') ? ownAddrs?.addresses[chain] : undefined;
  const destination = kind === 'wallet' ? (own?.address ?? '') : kind === 'swap' ? (from === 'custody' ? 'the market' : 'custody') : to;
  const amountUnits = max ? balance : toUnits(amount, decimals);
  const nothingHeld = BigInt(balance) === 0n;
  const canReview = Boolean(chosen && destination && amountUnits && BigInt(amountUnits) > 0n && BigInt(amountUnits) <= BigInt(balance));
  const sendKind =
    kind === 'transfer' ? 'transfer'
      : kind === 'swap' ? (from === 'custody' ? 'swap-to-market' : 'swap-to-custody')
        : from === 'custody' ? 'withdraw-custody' : 'withdraw';

  const doSend = async () => {
    setBusy(true);
    try {
      if (!amountUnits) throw new Error(`Amount must be a number with at most ${decimals} decimals.`);
      const r = await call<{ txId: string; ok: boolean; error: string | null }>('send', { kind: sendKind, asset: chosen, to: kind === 'swap' ? '' : destination, amount: amountUnits, memo, via: chosen === 'USDT' ? via : undefined });
      setResult(r.ok ? { txId: r.txId, ok: true, text: kind === 'transfer' ? 'Transferred.' : kind === 'swap' ? (from === 'custody' ? 'Moved to the market: the balance now trades on the book and in escrow.' : 'Moved back into your exchange\'s custody.') : kind === 'wallet' ? 'Withdrawal submitted. The observers pay it out in the next batch; the Wallet tab shows it when it lands.' : from === 'custody' ? 'Withdrawal submitted from your exchange\'s vault; its signer pays it out in the next batch.' : 'Withdrawal submitted. The observers pay it out in the next batch.' } : { txId: r.txId, ok: false, text: `Rejected: ${r.error ?? 'unknown'}` });
      setConfirm(false);
      setAmount('');
      setMax(false);
      setRefreshKey((k) => k + 1);
    } catch (e) {
      setResult({ txId: null, ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  };

  const verb = kind === 'transfer' ? 'Transfer' : kind === 'withdraw' ? 'Withdraw' : 'Move';
  const poolName = (p: Pool) => (p === 'custody' ? "your exchange's custody" : 'the market (Keel)');
  return (
    <div className="page">
      <div className="card">
        <div className="label">Send</div>
        <div className="row">
          <select value={kind} onChange={(e) => { setKind(e.target.value as Kind); setConfirm(false); setResult(null); setMax(false); }} title="Kind">
            <option value="transfer">Transfer on Keel</option>
            <option value="withdraw">Withdraw to another chain</option>
            {ownAddrs?.supported && <option value="wallet">Move to your wallet</option>}
            {hasCustody && <option value="swap">Custody ↔ market</option>}
          </select>
          {hasCustody && kind !== 'transfer' && (
            <select value={pool} onChange={(e) => { setPool(e.target.value as Pool); setMax(false); setConfirm(false); }} title="From">
              <option value="deposit">from the market</option>
              <option value="custody">from custody</option>
            </select>
          )}
          <select value={chosen} onChange={(e) => { setAsset(e.target.value); setMax(false); }} title="Asset">
            {options.map((o) => <option key={o.asset} value={o.asset}>{o.asset}</option>)}
          </select>
        </div>
        {chosen && (
          <p className="muted small">
            {from === 'custody' ? 'In custody' : 'In Keel'}: <b>{formatAmount(balance, decimals)} {chosen}</b>
            {chain && <> · {externalName(network.id, chain)}</>}
            {kind !== 'transfer' && nothingHeld && <> · nothing to {kind === 'wallet' ? 'move' : 'withdraw'} yet. Deposit {chosen} to Keel first, or buy it on the market.</>}
          </p>
        )}
        {kind === 'transfer' && <input value={to} placeholder="Recipient Keel address (64 hex)" onChange={(e) => setTo(e.target.value.trim())} />}
        {kind === 'withdraw' && chosen === 'USDT' && (
          <>
            <select value={via} onChange={(e) => { setVia(e.target.value); setConfirm(false); }} title="Network">
              {DOLLAR_RAILS.map((r) => <option key={r.via} value={r.via}>{externalName(network.id, r.chain)} · paid as {r.coin}</option>)}
            </select>
            <p className="muted small">Your USDT on Keel is one balance, backed by USDT held on every supported network. It is paid out as {rail?.coin ?? 'USDT'} on {chain ? externalName(network.id, chain) : 'that network'} in one step. A network whose reserve is running low charges a small premium, shown before you confirm; one with plenty charges none.</p>
          </>
        )}
        {kind === 'withdraw' && <input value={to} placeholder={chain ? `Destination address on ${externalName(network.id, chain)}` : 'Destination address on that chain'} onChange={(e) => setTo(e.target.value.trim())} />}
        {kind === 'swap' && (
          <p className="muted small">
            {from === 'custody'
              ? 'Moves the amount from your exchange\'s custody into the market side of your account, against your exchange\'s own floats; nothing moves on the external chain. Trade, then move back the same way.'
              : 'Moves the amount from the market side back into your exchange\'s custody, against its floats; nothing moves on the external chain.'}
          </p>
        )}
        {kind === 'wallet' && (
          <div>
            <div className="label">Your {chain ? externalName(network.id, chain) : ''} address</div>
            {own ? <code className="small">{own.address}</code> : <p className="muted small">Not available on this network.</p>}
            <p className="muted small">Only your seed phrase controls this address. The Wallet tab shows what it holds.</p>
          </div>
        )}
        <div className="row">
          <input value={max ? formatAmount(balance, decimals) : amount} disabled={max} placeholder={`Amount in ${chosen || 'units'}`} onChange={(e) => setAmount(e.target.value.trim())} />
          {kind !== 'transfer' && <label className="small"><input type="checkbox" checked={max} disabled={nothingHeld} onChange={(e) => setMax(e.target.checked)} /> Max</label>}
          {kind === 'transfer' && <input value={memo} placeholder="Memo (optional)" onChange={(e) => setMemo(e.target.value)} />}
        </div>
        {confirm ? (
          <div>
            <p className="small">
              {verb} <b>{max ? formatAmount(balance, decimals) : amount} {chosen}</b> to <code className="small">{destination}</code>
              {kind === 'swap' ? ` from ${poolName(from)}.` : kind !== 'transfer' ? `. Keel's withdrawal fee and the ${chain ? externalName(network.id, chain) : ''} network fee are taken from the amount.` : '?'}
            </p>
            <div className="row">
              <button disabled={busy} onClick={() => void doSend()}>{busy ? 'Signing…' : 'Confirm'}</button>
              <button className="mini" disabled={busy} onClick={() => setConfirm(false)}>Back</button>
            </div>
          </div>
        ) : (
          <button disabled={!canReview} onClick={() => { setResult(null); setConfirm(true); }}>Review</button>
        )}
        {amount && !max && amountUnits === null && <p className="error">Amount must be a number with at most {decimals} decimals.</p>}
        {amountUnits && BigInt(amountUnits) > BigInt(balance) && <p className="error">More than you hold {from === 'custody' ? 'in custody' : 'in Keel'}.</p>}
        {result && (
          <div className={result.ok ? 'small' : 'error'}>
            <div>{result.text}</div>
            {result.txId && (
              <div className="row small">
                <Address value={result.txId} />
                {explorerTxUrl(network, result.txId) && <a href={explorerTxUrl(network, result.txId)} target="_blank" rel="noreferrer">View in explorer ↗</a>}
              </div>
            )}
          </div>
        )}
        {error && <p className="error">{error}</p>}
      </div>
    </div>
  );
}
