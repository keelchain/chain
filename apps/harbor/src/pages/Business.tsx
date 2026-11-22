/**
 * The client's business settings: the retail fees it charges its own
 * users and what they have earned, and the custody tools for a client that
 * runs its own vault (generations, rotation to a fresh key, float swaps).
 */
import { useState, type FormEvent, type ReactNode } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { useCustody, useDecimals } from '../api/hooks';
import type { Custody } from '../api/types';
import { ActionResult, Field, Submit, useAction } from '../components/action';
import { Amount, Badge, Card, ErrorState, Hex, KV, Loading, Tabs } from '../components/ui';
import { toUnits } from '../lib/amounts';
import { formatBps, formatInt } from '../lib/format';
import { SCOPE_OPS, SCOPE_PAYOUTS, SCOPE_TEXT, authorizeProblem, parseKey, scopeLabels } from '../lib/serviceKeys';
import { useHref } from '../network/NetworkContext';
import { useHarbor, type Harbor } from './ClientGate';
import { day } from './shared';

type Tab = 'fees' | 'keys' | 'vaults' | 'rotate' | 'swap';
const TABS: { id: Tab; label: string }[] = [
  { id: 'fees', label: 'Retail fees' },
  { id: 'keys', label: 'Service keys' },
  { id: 'vaults', label: 'Vaults' },
  { id: 'rotate', label: 'Rotate' },
  { id: 'swap', label: 'Float swap' },
];

const CHAINS = ['Bitcoin', 'Ethereum', 'Tron', 'Bsc', 'Solana'] as const;
type Chain = (typeof CHAINS)[number];
const CHAIN_BY_CODE: Record<string, Chain> = { BTC: 'Bitcoin', ETH: 'Ethereum', TRON: 'Tron', BSC: 'Bsc', SOL: 'Solana' };
const CHAIN_NAME: Record<Chain, string> = { Bitcoin: 'Bitcoin', Ethereum: 'Ethereum', Tron: 'Tron', Bsc: 'BNB Smart Chain', Solana: 'Solana' };

/** The chain as the action names it, from the code (`BTC`) or the name the RPC returns. */
function chainOf(v: string): Chain | null {
  return CHAIN_BY_CODE[v] ?? (CHAINS.includes(v as Chain) ? (v as Chain) : null);
}
function chainName(v: string): string {
  const c = chainOf(v);
  return c ? CHAIN_NAME[c] : v;
}

// ---------------------------------------------------------------- service keys

/** The chain refuses a key that lasts longer than `sessions.max_secs` (30 days); stay a day under it. */
const MAX_KEY_DAYS = 29;

function ServiceKeys({ h }: { h: Harbor }) {
  const act = useAction();
  const revoke = useAction();
  const [text, setText] = useState('');
  const [ops, setOps] = useState(true);
  const [payouts, setPayouts] = useState(false);
  const [days, setDays] = useState(String(MAX_KEY_DAYS));
  const key = parseKey(text);
  const scope = (ops ? SCOPE_OPS : 0) | (payouts ? SCOPE_PAYOUTS : 0);
  const problem = authorizeProblem(key, scope, Number(days), h.address, MAX_KEY_DAYS);
  const sessions = (h.account.sessions ?? []).filter((s) => s.expires_at > h.nowSecs);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (problem || !key) return;
    const expires = Math.floor(Date.now() / 1000) + Number(days) * 86_400;
    void act.run(
      { AuthorizeSessionKey: { key, scope, expires_at: expires } },
      'Authorize a service key',
      `${key.slice(0, 8)}… may sign for this account until ${day(expires)}: ${scopeLabels(scope).join(' and ').toLowerCase()}.`,
    );
  };
  return (
    <div className="grid-2">
      <Card title="Authorize a service key">
        <form className="form" onSubmit={submit}>
          <p className="small text-2">
            The service that runs your exchange (your bridge or backend) signs with a key of its own, so it never holds this account's key and never competes with what you sign here. It creates the
            key; you paste its <strong>address</strong> below and approve in Keel Wallet. Never paste a secret here.
          </p>
          <Field label="The service's Keel address" hint={text !== '' && !key ? <span className="down">A Keel address is 64 hex characters.</span> : 'Ask whoever runs the service for it. It is public.'}>
            <input className="input mono" value={text} onChange={(e) => setText(e.target.value)} spellCheck={false} autoComplete="off" aria-invalid={text !== '' && !key} />
          </Field>
          <fieldset className="form">
            <legend className="small text-2">What it may do</legend>
            <label className="small">
              <input type="checkbox" checked={ops} onChange={(e) => setOps(e.target.checked)} /> <strong>Operations:</strong> {SCOPE_TEXT.find((s) => s.bit === SCOPE_OPS)!.detail}
            </label>
            <label className="small">
              <input type="checkbox" checked={payouts} onChange={(e) => setPayouts(e.target.checked)} /> <strong>Payouts:</strong> {SCOPE_TEXT.find((s) => s.bit === SCOPE_PAYOUTS)!.detail}
            </label>
          </fieldset>
          {payouts && <p className="small down">With payouts, whoever holds that key can send your users' funds out of your vault. Grant it only to a server you control.</p>}
          <Field label="Valid for (days)" hint={`At most ${MAX_KEY_DAYS}. Authorize it again before it runs out; the service stops when it does.`}>
            <input className="input" value={days} onChange={(e) => setDays(e.target.value.trim())} inputMode="numeric" />
          </Field>
          <p className="small text-2">It can never buy or change your plan, set your fees, unlock your KEEL or spend this account's own balance outside custody.</p>
          <div className="row">
            <Submit act={act} disabled={problem !== null}>
              Authorize in wallet
            </Submit>
            {problem && text !== '' && <span className="small text-2">{problem}</span>}
          </div>
          <ActionResult act={act} />
        </form>
      </Card>
      <Card title="Keys that can sign for this account" flush>
        {sessions.length === 0 ? (
          <p className="pad small text-2">None. Until you authorize one, only this account's own key signs.</p>
        ) : (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>Key</th>
                  <th>May do</th>
                  <th>Until</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {sessions.map((s) => (
                  <tr key={s.key}>
                    <td>
                      <Hex value={s.key} head={8} tail={6} />
                    </td>
                    <td>{scopeLabels(s.scope).join(', ') || `scope ${s.scope}`}</td>
                    <td>{day(s.expires_at)}</td>
                    <td className="num">
                      <button type="button" className="btn small" disabled={revoke.busy} onClick={() => void revoke.run({ RevokeSessionKey: { key: s.key } }, 'Revoke a service key', `${s.key.slice(0, 8)}… stops signing for this account at once.`)}>
                        Revoke
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="pad">
          <ActionResult act={revoke} />
        </div>
      </Card>
    </div>
  );
}

// ---------------------------------------------------------------- retail fees

const BPS = /^\d{1,5}$/;

function FeeInput({ label, value, onChange, cap }: { label: string; value: string; onChange: (v: string) => void; cap: number | null }) {
  const ok = BPS.test(value);
  const over = ok && cap !== null && Number(value) > cap;
  return (
    <Field
      label={`${label} (basis points)`}
      hint={over ? <span className="down">Above the cap of {cap} bps.</span> : ok ? `${formatBps(value)} of the amount${cap !== null ? ` · cap ${cap} bps` : ''}` : 'A whole number of basis points; 100 = 1%.'}
    >
      <input className="input" value={value} onChange={(e) => onChange(e.target.value)} inputMode="numeric" pattern="[0-9]{1,5}" required aria-invalid={!ok || over} />
    </Field>
  );
}

function Fees({ h }: { h: Harbor }) {
  const act = useAction();
  const c = h.client;
  const [p2p, setP2p] = useState(String(c.fee?.p2p_bps ?? 0));
  const [taker, setTaker] = useState(String(c.fee?.taker_bps ?? 0));
  const [withdraw, setWithdraw] = useState(String(c.fee?.withdraw_bps ?? 0));
  // With the protocol fee waived the schedule is the client's own business; otherwise governance caps it.
  const caps = c.protocol_fee_waived ? null : c.caps;
  const fields = [p2p, taker, withdraw];
  const valid = fields.every((v) => BPS.test(v)) && (!caps || (Number(p2p) <= caps.p2p_bps && Number(taker) <= caps.taker_bps && Number(withdraw) <= caps.withdraw_bps));
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!valid) return;
    void act.run(
      { SetClientFee: { p2p_bps: Number(p2p), taker_bps: Number(taker), withdraw_bps: Number(withdraw) } },
      'Set your retail fees',
      `P2P ${formatBps(p2p)}, taker ${formatBps(taker)}, withdrawal ${formatBps(withdraw)}, charged to the accounts you attested.`,
    );
  };
  return (
    <div className="grid-2">
      <Card title="Retail fees">
        <form className="form" onSubmit={submit}>
          <p className="small text-2">
            What the accounts you attested pay you, taken from the payer and credited to this account in the same posting.{' '}
            {caps ? 'Your users pay these on top of the protocol fee, and governance caps them.' : 'While your plan runs they pay no protocol fee, so this is all they pay; there is no cap.'}
          </p>
          <FeeInput label="P2P escrow release" value={p2p} onChange={setP2p} cap={caps?.p2p_bps ?? null} />
          <FeeInput label="Order-book fill (taker)" value={taker} onChange={setTaker} cap={caps?.taker_bps ?? null} />
          <FeeInput label="Withdrawal" value={withdraw} onChange={setWithdraw} cap={caps?.withdraw_bps ?? null} />
          <div className="row">
            <Submit act={act} disabled={!valid}>
              Save fees
            </Submit>
            <span className="small text-2">{c.fee ? `Now: ${formatBps(c.fee.p2p_bps)} / ${formatBps(c.fee.taker_bps)} / ${formatBps(c.fee.withdraw_bps)}` : 'No retail fee is set yet.'}</span>
          </div>
          <ActionResult act={act} />
        </form>
      </Card>
      <div className="stack">
        <Card title="Earned" flush>
          {c.earned.length === 0 ? (
            <div className="empty">No retail fees earned yet.</div>
          ) : (
            <div className="table-wrap">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Asset</th>
                    <th className="num">Retail fees earned</th>
                  </tr>
                </thead>
                <tbody>
                  {c.earned.map((e) => (
                    <tr key={e.asset}>
                      <td>{e.asset}</td>
                      <td className="num">
                        <Amount value={e.amount} asset={e.asset} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          <div className="card-body small text-2">Totals since the account became a client; each fee is already in your balance in the asset it was paid in.</div>
        </Card>
        <Card title="Attested accounts">
          <div className="big-figure">{formatInt(c.attested_accounts)}</div>
          <p className="small text-2">Accounts whose attestation names you. They pay your retail fees, and while your plan runs their protocol fees are waived and their actions can come out of your daily pool.</p>
        </Card>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- custody

/** `RegisterCustodyVault`: the first vault on a network (a later key change is a rotation). */
function RegisterVault({ c }: { c: Custody }) {
  const act = useAction();
  const taken = new Set(c.vaults.map((v) => chainOf(v.chain)));
  const free = CHAINS.filter((x) => !taken.has(x));
  const [chain, setChain] = useState<Chain>(free[0] ?? 'Bitcoin');
  const [pk, setPk] = useState('');
  const [cc, setCc] = useState('');
  const [url, setUrl] = useState('');
  if (free.length === 0) return null;
  const picked = free.includes(chain) ? chain : free[0]!;
  // Solana's vault key is 32 bytes (Ed25519); the others are 33-byte compressed secp256k1 keys.
  const keyOk = picked === 'Solana' ? /^[0-9a-fA-F]{64}$/.test(pk.trim()) : /^[0-9a-fA-F]{66}$/.test(pk.trim());
  const ok = keyOk && /^[0-9a-fA-F]{64}$/.test(cc.trim()) && /^https?:\/\/\S+$/.test(url.trim());
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ok) return;
    void act.run(
      { RegisterCustodyVault: { chain: picked, epoch: 1, public_key: pk.trim().toLowerCase(), chain_code: cc.trim().toLowerCase(), signer_url: url.trim() } },
      `Register your ${CHAIN_NAME[picked]} vault`,
      'Deposit addresses for your users are derived from this key, and your signer at that URL signs their payouts.',
    );
  };
  return (
    <Card title="Register a vault">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">
          Your signer prints the public key and chain code when it starts (<span className="mono">keel-tss serve</span>, or your own KMS hook). You keep the key; Keel only learns its public half and
          where to ask for signatures. One vault per network; to change a key later, use Rotate.
        </p>
        <Field label="Network">
          <select className="select" value={picked} onChange={(e) => setChain(e.target.value as Chain)}>
            {free.map((x) => (
              <option key={x} value={x}>
                {CHAIN_NAME[x]}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Public key" hint={picked === 'Solana' ? '32 bytes, hex (Ed25519)' : '33 bytes, hex (compressed)'}>
          <input className="input mono" value={pk} onChange={(e) => setPk(e.target.value)} spellCheck={false} />
        </Field>
        <Field label="Chain code" hint="32 bytes, hex">
          <input className="input mono" value={cc} onChange={(e) => setCc(e.target.value)} spellCheck={false} />
        </Field>
        <Field label="Signer URL" hint="Reachable by the observers that build your payout batches.">
          <input className="input mono" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://signer.example.com" />
        </Field>
        <div className="row">
          <Submit act={act} disabled={!ok}>
            Register
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

/** `SetCustodyOptions`: whether deposits to the client's users wait for its release. */
function Screening({ c }: { c: Custody }) {
  const act = useAction();
  const on = c.hold_deposits === true;
  return (
    <Card title="Deposit screening">
      <div className="form">
        <p className="small text-2">
          {on
            ? 'On: a deposit to one of your users is credited on hold. It is in your vault and owed, and the user cannot spend it until you release it under Users.'
            : 'Off: a deposit to one of your users is credited to their balance at once. Turn it on to screen deposits first; each then waits for your release under Users.'}{' '}
          Your own top-ups are never held.
        </p>
        <div className="row">
          <button type="button" className="btn" disabled={act.busy} onClick={() => void act.run({ SetCustodyOptions: { hold_deposits: !on } }, on ? 'Credit deposits at once' : 'Hold deposits for release')}>
            {on ? 'Turn off: credit at once' : 'Turn on: hold deposits'}
          </button>
        </div>
        <ActionResult act={act} />
      </div>
    </Card>
  );
}

function Vaults({ c }: { c: Custody }) {
  const act = useAction();
  return (
    <>
      {c.rotations.length > 0 && (
        <Card title="Pending rotations">
          <div className="stack-s">
            {c.rotations.map((r) => {
              const chain = chainOf(r.chain);
              return (
                <div key={`${r.chain}:${r.gen}`} className="row between">
                  <span>
                    {chainName(r.chain)} → generation {r.gen} at height {formatInt(r.activate_at)} · signer <span className="mono small">{r.signer_url}</span>
                  </span>
                  <button type="button" className="btn small" disabled={act.busy || !chain} onClick={() => chain && void act.run({ CancelCustodyRotation: { chain } }, `Cancel the scheduled ${chainName(r.chain)} rotation`)}>
                    Cancel
                  </button>
                </div>
              );
            })}
            <ActionResult act={act} />
          </div>
        </Card>
      )}
      {c.vaults.length === 0 && (
        <Card title="Custody vaults">
          <p className="text-2">No custody vault is registered for this account. A client that holds its users' coins under its own key registers one below, once its signer is up.</p>
        </Card>
      )}
      <RegisterVault c={c} />
      {c.vaults.length > 0 && <Screening c={c} />}
      {c.vaults.map((v) => (
        <Card
          key={`${v.chain}:${v.gen}`}
          title={
            <h2>
              {chainName(v.chain)} vault · generation {v.gen}
            </h2>
          }
          actions={
            <span className="row">
              <Badge tone={v.status === 'Active' ? 'good' : v.status === 'Draining' ? 'warn' : 'neutral'}>{v.status}</Badge>
              {v.status === 'Draining' && <Badge>empties through payouts</Badge>}
              {v.status === 'Draining' && chainOf(v.chain) && (
                <button
                  type="button"
                  className="btn small"
                  disabled={act.busy}
                  title="Once it is empty and its grace is over"
                  onClick={() => void act.run({ RetireCustodyGeneration: { chain: chainOf(v.chain), custodian: c.custodian, gen: v.gen } }, `Retire ${chainName(v.chain)} generation ${v.gen}`, 'Refused until the generation is empty and its grace has passed.')}
                >
                  Retire
                </button>
              )}
            </span>
          }
          flush
        >
          <KV
            rows={[
              ['Key', <Hex value={v.vault.public_key} head={12} tail={8} />],
              ['Signer', <span className="mono small">{v.signer_url}</span>],
              ['Own address (index 0)', v.address ? <span className="mono small">{v.address}</span> : '—'],
              ['Deposit indexes', formatInt(v.next_deposit_index - 1)],
              ...(v.status === 'Draining'
                ? [['Grace ends at height', formatInt(v.drain_until)] as [ReactNode, ReactNode], ['Observers reporting empty', formatInt(v.reported_empty)] as [ReactNode, ReactNode]]
                : []),
            ]}
          />
          {v.reserves.length > 0 && (
            <div className="table-wrap">
              <table className="tbl">
                <thead>
                  <tr>
                    <th>Asset</th>
                    <th className="num">Reserve</th>
                    <th className="num">Liabilities</th>
                    <th>State</th>
                  </tr>
                </thead>
                <tbody>
                  {v.reserves.map((r) => (
                    <tr key={r.asset}>
                      <td>{r.asset}</td>
                      <td className="num">
                        <Amount value={r.reserve} asset={r.asset} />
                      </td>
                      <td className="num">
                        <Amount value={r.liabilities} asset={r.asset} />
                      </td>
                      <td>{r.halted ? <Badge tone="bad">Halted</Badge> : <Badge tone="good">Covered</Badge>}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
      ))}
      {c.floats.length > 0 && (
        <Card title="Floats" flush>
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>Asset</th>
                  <th className="num">In your vault (custody)</th>
                  <th className="num">In the network vault (deposit)</th>
                </tr>
              </thead>
              <tbody>
                {c.floats.map((f) => (
                  <tr key={f.asset}>
                    <td>{f.asset}</td>
                    <td className="num">
                      <Amount value={f.custody} asset={f.asset} />
                    </td>
                    <td className="num">
                      <Amount value={f.deposit} asset={f.asset} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="card-body small text-2">What a float swap can draw on in each direction; {formatInt(c.accounts)} accounts in your custody.</div>
        </Card>
      )}
    </>
  );
}

function Rotate() {
  const act = useAction();
  const [chain, setChain] = useState<Chain>('Bitcoin');
  const [pk, setPk] = useState('');
  const [cc, setCc] = useState('');
  const [url, setUrl] = useState('');
  const [at, setAt] = useState('');
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void act.run(
      { RotateCustodyVault: { chain, public_key: pk.trim(), chain_code: cc.trim(), signer_url: url.trim(), activate_at: at.trim() ? Number(at) : null } },
      `Rotate the ${CHAIN_NAME[chain]} vault to a fresh key`,
    );
  };
  return (
    <Card title="Rotate to a fresh pool">
      <p className="text-2 small">
        Start a second signer for a new key (its <span className="mono">keel-tss pubkey</span> prints the key and chain code), then register it here. Every account gets a new deposit address on that
        chain; Keel accounts and balances do not change. The old key keeps crediting its addresses and paying withdrawals until it is empty and its grace is over. The two generations are separate
        pools: nothing is ever moved from the old key to the new one.
      </p>
      <form className="form" onSubmit={submit}>
        <Field label="Chain">
          <select className="select" value={chain} onChange={(e) => setChain(e.target.value as Chain)}>
            {CHAINS.map((c) => (
              <option key={c} value={c}>
                {CHAIN_NAME[c]}
              </option>
            ))}
          </select>
        </Field>
        <Field label="New public key" hint="33 bytes, hex">
          <input className="input mono" value={pk} onChange={(e) => setPk(e.target.value)} pattern="[0-9a-fA-F]{66}" required />
        </Field>
        <Field label="Chain code" hint="32 bytes, hex">
          <input className="input mono" value={cc} onChange={(e) => setCc(e.target.value)} pattern="[0-9a-fA-F]{64}" required />
        </Field>
        <Field label="Signer URL" hint="The new key's signer; keep the old signer running for the draining generation.">
          <input className="input mono" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://signer-2.example.com" required />
        </Field>
        <Field label="Activate at height" hint="Empty = at once">
          <input className="input" value={at} onChange={(e) => setAt(e.target.value)} inputMode="numeric" pattern="[0-9]*" />
        </Field>
        <div className="row">
          <Submit act={act}>Rotate</Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function Swap({ c }: { c: Custody }) {
  const act = useAction();
  const dec = useDecimals();
  const assets = c.floats.map((f) => f.asset);
  const [asset, setAsset] = useState(assets[0] ?? 'BTC.BTC');
  const [amount, setAmount] = useState('');
  const [to, setTo] = useState<'deposit' | 'custody'>('deposit');
  const [behalf, setBehalf] = useState('');
  const units = toUnits(amount, dec(asset));
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (units === null || units === 0n) return;
    void act.run(
      { SwapCustody: { asset, amount: units, to, on_behalf_of: behalf.trim() ? behalf.trim() : null } },
      to === 'deposit' ? `Swap ${amount} ${asset} from custody into the market` : `Swap ${amount} ${asset} back into custody`,
    );
  };
  return (
    <Card title="Float swap">
      <p className="text-2 small">
        Move a balance between your custody and the network vault against your own floats, in one posting, with no coins moving on any external chain. Into the market (deposit) it trades on the book
        and escrow; back into custody it leaves. Each swap is charged the payout price. Sign for one of your accounts with the field below.
      </p>
      <form className="form" onSubmit={submit}>
        <Field label="Asset">
          <input className="input mono" list="swap-assets" value={asset} onChange={(e) => setAsset(e.target.value)} required />
          <datalist id="swap-assets">
            {assets.map((a) => (
              <option key={a} value={a} />
            ))}
          </datalist>
        </Field>
        <Field label="Amount" hint={amount !== '' && units === null ? <span className="down">Not an amount of {asset}.</span> : undefined}>
          <input className="input" value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" required aria-invalid={amount !== '' && units === null} />
        </Field>
        <Field label="Direction">
          <select className="select" value={to} onChange={(e) => setTo(e.target.value as 'deposit' | 'custody')}>
            <option value="deposit">custody → market (deposit)</option>
            <option value="custody">market (deposit) → custody</option>
          </select>
        </Field>
        <Field label="On behalf of" hint="An account in your custody (64 hex); empty = your own balance">
          <input className="input mono" value={behalf} onChange={(e) => setBehalf(e.target.value)} pattern="[0-9a-fA-F]{64}" />
        </Field>
        <div className="row">
          <Submit act={act} disabled={units === null || units === 0n}>
            Swap
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function CustodyTab({ h, tab }: { h: Harbor; tab: Exclude<Tab, 'fees' | 'keys'> }) {
  const custody = useCustody(h.address);
  if (tab === 'rotate') return <Rotate />;
  if (custody.isError) return <ErrorState error={custody.error} what="custody vaults" />;
  if (!custody.data) {
    return (
      <Card>
        <Loading rows={5} />
      </Card>
    );
  }
  return tab === 'vaults' ? <Vaults c={custody.data} /> : <Swap c={custody.data} />;
}

export function BusinessPage() {
  const h = useHarbor();
  const href = useHref();
  const navigate = useNavigate();
  const { tab: param } = useParams();
  const tab: Tab = TABS.find((t) => t.id === param)?.id ?? 'fees';
  return (
    <>
      <div className="page-head">
        <h1>Business</h1>
        <span className="sub">Your fees, your users and your vaults.</span>
      </div>
      {h.client.attester ? (
        <>
          <Tabs label="Business sections" tabs={TABS} value={tab} onChange={(t) => navigate(href(t === 'fees' ? '/business' : `/business/${t}`))} />
          {tab === 'fees' ? <Fees h={h} /> : tab === 'keys' ? <ServiceKeys h={h} /> : <CustodyTab h={h} tab={tab} />}
        </>
      ) : (
        <Card>
          <p className="text-2">Retail fees, attested users and vaults belong to a registered client. They open here, for this same account, once it is registered.</p>
        </Card>
      )}
    </>
  );
}
