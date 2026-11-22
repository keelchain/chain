/**
 * A client's users: look an account up, onboard it (attestation, deposit
 * address), move its custody balance, pay it out, and release deposits
 * held for screening. Each is one action signed in Keel Wallet; nothing
 * here needs the CLI or a key of the user's.
 */
import { useState, type FormEvent } from 'react';
import { useAccount, useCustody, useDecimals } from '../api/hooks';
import type { AccountRecord } from '../api/types';
import { ActionResult, Field, Submit, useAction } from '../components/action';
import { Amount, Card, ErrorState, Hex, Loading, Notice } from '../components/ui';
import { big, toUnits } from '../lib/amounts';
import { parseKey } from '../lib/serviceKeys';
import { CHAIN_NAME, chainOf, payoutRails, type Chain } from '../lib/users';
import { useHarbor, type Harbor } from './ClientGate';
import { day } from './shared';

function held(a: AccountRecord, type: string): { asset: string; balance: bigint }[] {
  return a.balances.filter((b) => b.account_type === type && big(b.balance) > 0n).map((b) => ({ asset: b.asset, balance: big(b.balance) }));
}

function AssetSelect({ value, onChange, options }: { value: string; onChange: (v: string) => void; options: string[] }) {
  return (
    <select className="select" value={value} onChange={(e) => onChange(e.target.value)}>
      {options.map((a) => (
        <option key={a} value={a}>
          {a}
        </option>
      ))}
    </select>
  );
}

function Attest({ user }: { user: string }) {
  const act = useAction();
  const [tier, setTier] = useState('1');
  const [days, setDays] = useState('365');
  const ok = /^[0-9]{1,3}$/.test(tier) && /^[0-9]{1,4}$/.test(days) && Number(days) > 0;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ok) return;
    const expires = Math.floor(Date.now() / 1000) + Number(days) * 86_400;
    void act.run({ Attest: { subject: user, tier: Number(tier), expires_at: expires } }, 'Attest a user', `Tier ${tier} for ${user.slice(0, 8)}…, valid until ${day(expires)}. It becomes your user: your fees and your plan apply to it.`);
  };
  return (
    <Card title="Attest (onboard or renew)">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">Your statement that you verified this account to a KYC tier. It makes the account your user. An attestation lapses; renew it here before it does.</p>
        <Field label="KYC tier" hint="Your own scale; offers can require a minimum.">
          <input className="input" value={tier} onChange={(e) => setTier(e.target.value.trim())} inputMode="numeric" />
        </Field>
        <Field label="Valid for (days)">
          <input className="input" value={days} onChange={(e) => setDays(e.target.value.trim())} inputMode="numeric" />
        </Field>
        <div className="row">
          <Submit act={act} disabled={!ok}>
            Attest
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function DepositAddress({ h, user, chains }: { h: Harbor; user: string; chains: Chain[] }) {
  const act = useAction();
  const [chain, setChain] = useState<Chain>(chains[0] ?? 'Bitcoin');
  if (chains.length === 0) return null;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void act.run({ RequestCustodyAddress: { chain, custodian: h.address, owner: user === h.address ? null : user } }, `Issue a ${CHAIN_NAME[chain]} deposit address`, `In your vault, for ${user.slice(0, 8)}…. Asking again returns the same address.`);
  };
  return (
    <Card title="Deposit address in your vault">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">The address is derived from your vault key; coins sent to it are credited to this user's custody balance. The receipt shows its index, and the address itself is listed under Business, Vaults.</p>
        <Field label="Network">
          <select className="select" value={chain} onChange={(e) => setChain(e.target.value as Chain)}>
            {chains.map((c) => (
              <option key={c} value={c}>
                {CHAIN_NAME[c]}
              </option>
            ))}
          </select>
        </Field>
        <div className="row">
          <Submit act={act}>Issue address</Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function Transfer({ h, user, a }: { h: Harbor; user: string; a: AccountRecord }) {
  const act = useAction();
  const dec = useDecimals();
  const rows = held(a, 'custody');
  const [asset, setAsset] = useState(rows[0]?.asset ?? '');
  const [amount, setAmount] = useState('');
  const [to, setTo] = useState('');
  const [memo, setMemo] = useState('');
  if (rows.length === 0) return null;
  const chosen = rows.find((r) => r.asset === asset) ?? rows[0]!;
  const units = toUnits(amount, dec(chosen.asset));
  const dest = to.trim() === '' ? h.address : parseKey(to);
  const ok = units !== null && units > 0n && units <= chosen.balance && dest !== null && dest !== user;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ok || !dest || units === null) return;
    void act.run(
      { TransferCustody: { legs: [{ from: null, to: dest, asset: chosen.asset, amount: units }], on_behalf_of: user === h.address ? null : user, memo: memo.trim() || null } },
      `Move ${amount} ${chosen.asset} inside your custody`,
      `From ${user.slice(0, 8)}… to ${dest.slice(0, 8)}…. No coins move on any network.`,
    );
  };
  return (
    <Card title="Move inside custody">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">To another of your users, or to your own account (an escrow, a fee, a correction). One ledger posting, free.</p>
        <Field label="Asset">
          <AssetSelect value={chosen.asset} onChange={setAsset} options={rows.map((r) => r.asset)} />
        </Field>
        <Field label="Amount" hint={<>Holds <Amount value={chosen.balance} asset={chosen.asset} />.</>}>
          <input className="input" value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" />
        </Field>
        <Field label="To" hint="A Keel address of one of your users. Empty = your own account.">
          <input className="input mono" value={to} onChange={(e) => setTo(e.target.value)} spellCheck={false} />
        </Field>
        <Field label="Reference" hint="Optional, shown in the event.">
          <input className="input" value={memo} onChange={(e) => setMemo(e.target.value)} maxLength={64} />
        </Field>
        <div className="row">
          <Submit act={act} disabled={!ok}>
            Move
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function PayOut({ h, user, a }: { h: Harbor; user: string; a: AccountRecord }) {
  const act = useAction();
  const dec = useDecimals();
  const rows = held(a, 'custody');
  const [asset, setAsset] = useState(rows[0]?.asset ?? '');
  const [via, setVia] = useState('');
  const [amount, setAmount] = useState('');
  const [to, setTo] = useState('');
  const [fee, setFee] = useState('');
  if (rows.length === 0) return null;
  const chosen = rows.find((r) => r.asset === asset) ?? rows[0]!;
  const rails = payoutRails(chosen.asset, h.client.billing_asset);
  const rail = rails === null ? null : (rails.find((r) => r === via) ?? rails[0] ?? null);
  const d = dec(chosen.asset);
  const units = toUnits(amount, d);
  const feeUnits = fee.trim() === '' ? 0n : toUnits(fee, d);
  const ok = units !== null && units > 0n && feeUnits !== null && units + feeUnits <= chosen.balance && to.trim().length >= 20 && (rails === null || rail !== null);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ok || units === null || feeUnits === null) return;
    void act.run(
      { WithdrawCustodyFor: { owner: user, asset: chosen.asset, via: rail, to: to.trim(), amount: units, fee: feeUnits > 0n ? { to: h.address, amount: feeUnits } : null } },
      `Pay out ${amount} ${chosen.asset}${rail ? ` as ${rail}` : ''}`,
      `From ${user.slice(0, 8)}… to ${to.trim()}, through your vault and your signer.${feeUnits > 0n ? ` Your fee of ${fee} ${chosen.asset} is charged on top.` : ''}`,
    );
  };
  return (
    <Card title="Pay out">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">A withdrawal to an external address, paid from your vault by your own signer. The network fee comes out of your gas there; the service price out of your plan.</p>
        <Field label="Asset">
          <AssetSelect value={chosen.asset} onChange={setAsset} options={rows.map((r) => r.asset)} />
        </Field>
        {rails !== null && (
          <Field label="Network to pay on" hint="Your vault must hold enough of that network's coin; if it does not, the payout is refused and you pick another.">
            <AssetSelect value={rail ?? ''} onChange={setVia} options={rails} />
          </Field>
        )}
        <Field label="Amount" hint={<>Holds <Amount value={chosen.balance} asset={chosen.asset} />.</>}>
          <input className="input" value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" />
        </Field>
        <Field label="Destination address" hint="On the external network. Check it: a payout cannot be recalled.">
          <input className="input mono" value={to} onChange={(e) => setTo(e.target.value)} spellCheck={false} />
        </Field>
        <Field label="Your fee on top" hint="Optional. Charged to the user and paid to your account in the same posting.">
          <input className="input" value={fee} onChange={(e) => setFee(e.target.value)} inputMode="decimal" />
        </Field>
        <div className="row">
          <Submit act={act} disabled={!ok}>
            Pay out
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function Release({ h, user, a }: { h: Harbor; user: string; a: AccountRecord }) {
  const act = useAction();
  const dec = useDecimals();
  const rows = held(a, 'custody_hold');
  const [asset, setAsset] = useState(rows[0]?.asset ?? '');
  const [amount, setAmount] = useState('');
  const [keep, setKeep] = useState(false);
  if (rows.length === 0) return null;
  const chosen = rows.find((r) => r.asset === asset) ?? rows[0]!;
  const units = amount.trim() === '' ? chosen.balance : toUnits(amount, dec(chosen.asset));
  const ok = units !== null && units > 0n && units <= chosen.balance;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!ok || units === null) return;
    void act.run(
      { ReleaseCustodyHold: { owner: user, asset: chosen.asset, amount: units, to: keep ? h.address : null } },
      keep ? `Take a held ${chosen.asset} deposit to your own account` : `Release a held ${chosen.asset} deposit to the user`,
      keep ? 'The user does not receive it. What you then do with it is yours to decide and to answer for.' : 'The user can spend it from that block.',
    );
  };
  return (
    <Card title="Held deposits">
      <form className="form" onSubmit={submit}>
        <Notice tone="warn" title="This user has deposits on hold.">
          They are in your vault and owed, but the user cannot spend them until you release them.
        </Notice>
        <Field label="Asset">
          <AssetSelect value={chosen.asset} onChange={setAsset} options={rows.map((r) => r.asset)} />
        </Field>
        <Field label="Amount" hint={<>On hold: <Amount value={chosen.balance} asset={chosen.asset} />. Empty = all of it.</>}>
          <input className="input" value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" />
        </Field>
        <label className="small">
          <input type="checkbox" checked={keep} onChange={(e) => setKeep(e.target.checked)} /> Do not give it to the user: move it to my own account (a flagged deposit)
        </label>
        <div className="row">
          <Submit act={act} disabled={!ok}>
            {keep ? 'Move to my account' : 'Release to the user'}
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function UserPanel({ h, user }: { h: Harbor; user: string }) {
  const account = useAccount(user);
  const custody = useCustody(h.address);
  if (account.isError) return <ErrorState error={account.error} what="account" />;
  if (!account.data) {
    return (
      <Card>
        <Loading rows={4} />
      </Card>
    );
  }
  const a = account.data;
  const mine = user === h.address;
  const yours = mine || a.attester === h.address || a.custodian === h.address;
  const elsewhere = !mine && a.custodian && a.custodian !== h.address;
  const live = (a.tier_expires_at ?? 0) > h.nowSecs;
  const chains = Array.from(new Set((custody.data?.vaults ?? []).filter((v) => v.status === 'Active').map((v) => v.chain)))
    .map((c) => chainOf(c))
    .filter((c): c is Chain => c !== null);
  const balances = [...held(a, 'custody').map((r) => ({ ...r, kind: 'Spendable' })), ...held(a, 'custody_hold').map((r) => ({ ...r, kind: 'On hold' })), ...held(a, 'custody_escrow').map((r) => ({ ...r, kind: 'Being paid out' }))];
  return (
    <div className="stack">
      <Card title={mine ? 'Your own account' : 'User'} flush>
        <div className="pad">
          <p>
            <Hex value={user} head={10} tail={8} />{' '}
            {mine ? null : a.attester === h.address ? (live ? `· your user, tier ${a.tier ?? 0}, attested until ${day(a.tier_expires_at ?? 0)}` : '· your user, attestation lapsed: renew it below') : a.attester ? '· attested by another client' : '· not attested by anyone'}
          </p>
          {elsewhere && <p className="small down">This account's custody balances are in another client's vault. You can attest it, but not hold or move custody for it.</p>}
        </div>
        {balances.length > 0 && (
          <div className="table-wrap">
            <table className="tbl">
              <thead>
                <tr>
                  <th>In your custody</th>
                  <th />
                  <th className="num">Amount</th>
                </tr>
              </thead>
              <tbody>
                {balances.map((b) => (
                  <tr key={`${b.kind}:${b.asset}`}>
                    <td>{b.asset}</td>
                    <td>{b.kind}</td>
                    <td className="num">
                      <Amount value={b.balance} asset={b.asset} sym={false} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {balances.length === 0 && <p className="pad small text-2">No custody balance.</p>}
      </Card>
      <div className="grid-2">
        {!mine && <Attest user={user} />}
        {yours && !elsewhere && <DepositAddress h={h} user={user} chains={chains} />}
        {yours && !elsewhere && <Release h={h} user={user} a={a} />}
        {yours && !elsewhere && <Transfer h={h} user={user} a={a} />}
        {yours && !elsewhere && <PayOut h={h} user={user} a={a} />}
      </div>
      {yours && !elsewhere && chains.length === 0 && <p className="small text-2">Deposit addresses, transfers and payouts need a custody vault: register one under Business, Vaults.</p>}
    </div>
  );
}

export function UsersPage() {
  const h = useHarbor();
  const [text, setText] = useState('');
  const [user, setUser] = useState<string | null>(null);
  const key = parseKey(text);
  if (!h.client.attester) {
    return (
      <>
        <div className="page-head">
          <h1>Users</h1>
        </div>
        <Card>
          <p className="text-2">Onboarding and running users belongs to a registered client. It opens here, for this same account, once it is registered.</p>
        </Card>
      </>
    );
  }
  return (
    <>
      <div className="page-head">
        <h1>Users</h1>
        <span className="sub">{h.client.attested_accounts.toLocaleString('en-US')} attested accounts. Look one up to onboard it, move its balance or pay it out.</span>
      </div>
      <Card>
        <form
          className="form"
          onSubmit={(e) => {
            e.preventDefault();
            if (key) setUser(key);
          }}
        >
          <Field label="Keel address of the user" hint={text !== '' && !key ? <span className="down">A Keel address is 64 hex characters.</span> : 'An account needs no key of its own to be your user: any address works, including one you derived for a customer.'}>
            <input className="input mono" value={text} onChange={(e) => setText(e.target.value)} spellCheck={false} autoComplete="off" />
          </Field>
          <div className="row">
            <button type="submit" className="btn primary" disabled={!key}>
              Open
            </button>
            <button type="button" className="btn" onClick={() => { setText(h.address); setUser(h.address); }}>
              My own account
            </button>
          </div>
        </form>
      </Card>
      {user && <UserPanel key={user} h={h} user={user} />}
    </>
  );
}
