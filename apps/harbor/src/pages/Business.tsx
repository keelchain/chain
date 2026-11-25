/**
 * The client's business settings: the retail fees it charges its own
 * users and what they have earned, and the custody tools for a client that
 * runs its own vault (generations, rotation to a fresh key, float swaps).
 */
import { Keypair } from '@keelchain/sdk';
import { useQueryClient } from '@tanstack/react-query';
import { useState, type FormEvent, type ReactNode } from 'react';
import { useNavigate, useParams } from 'react-router-dom';
import { useClients, useCustody, useDecimals } from '../api/hooks';
import type { Custody } from '../api/types';
import { ActionResult, Field, Submit, useAction } from '../components/action';
import { Amount, Badge, Card, ErrorState, Hex, KV, Loading, Notice, Tabs } from '../components/ui';
import { toUnits } from '../lib/amounts';
import { formatBps, formatInt } from '../lib/format';
import { LOGO_MAX, NAME_MAX, hostOf, logoProblem, nameProblem, proofUrl, websiteProblem } from '../lib/profile';
import { SCOPE_OPS, SCOPE_PAYOUTS, SCOPE_TEXT, authorizeProblem, hexOf, parseKey, requestFromQuery, scopeLabels } from '../lib/serviceKeys';
import { useHref, useNetwork } from '../network/NetworkContext';
import { explorerUrl } from '../network/networks';
import { useWallet } from '../wallet/WalletContext';
import { useHarbor, type Harbor } from './ClientGate';
import { day } from './shared';

type Tab = 'fees' | 'profile' | 'keys' | 'vaults' | 'rotate' | 'swap';
const TABS: { id: Tab; label: string }[] = [
  { id: 'fees', label: 'Retail fees' },
  { id: 'profile', label: 'Public profile' },
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
  // A link from the service's operator carries its address and what it asks for.
  const [asked] = useState(() => requestFromQuery(window.location.search));
  const [text, setText] = useState(asked?.key ?? '');
  const [ops, setOps] = useState(asked ? (asked.scope & SCOPE_OPS) !== 0 : true);
  const [payouts, setPayouts] = useState(asked ? (asked.scope & SCOPE_PAYOUTS) !== 0 : false);
  const [days, setDays] = useState(String(MAX_KEY_DAYS));
  // A key made on this page: its secret is held in memory only, shown once, never stored or sent.
  const [made, setMade] = useState<{ address: string; secret: string } | null>(null);
  const [copied, setCopied] = useState(false);
  const create = () => {
    const kp = Keypair.random();
    setMade({ address: kp.address, secret: hexOf(kp.secret) });
    setText(kp.address);
    setCopied(false);
  };
  const copySecret = () => {
    if (!made) return;
    void navigator.clipboard?.writeText(made.secret).then(() => setCopied(true));
  };
  const key = parseKey(text);
  const scope = (ops ? SCOPE_OPS : 0) | (payouts ? SCOPE_PAYOUTS : 0);
  const sessions = (h.account.sessions ?? []).filter((s) => s.expires_at > h.nowSecs);
  // A secret looks exactly like an address (64 hex), and authorizing one
  // would publish it on the chain. Catch the two cases the page can see:
  // the secret it just created, and the secret of a key already listed.
  const isSecret =
    key !== null &&
    (key === made?.secret ||
      (() => {
        try {
          const a = Keypair.fromSecret(key).address;
          return a === made?.address || sessions.some((s) => s.key === a);
        } catch {
          return false;
        }
      })());
  const problem = isSecret ? 'That is a secret, not an address. Do not authorize it.' : authorizeProblem(key, scope, Number(days), h.address, MAX_KEY_DAYS);
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
    <div className="stack">
      <Card title="How a service key works">
        <div className="prose small">
          <p>
            Your exchange's backend (a bridge, a bot, a server) has to sign actions for this account: attest users, issue deposit addresses, move balances, pay out. It should not hold this
            account's own key. So it gets <strong>a key of its own</strong>, and you tell the chain, once, that this key may act for you, for what, and until when.
          </p>
          <ol>
            <li>
              <strong>The service gets a key.</strong> Either it already has one (whoever runs it gives you its <em>address</em>, or a link that fills this page in), or you create one here with the
              button below and put its <em>secret</em> into the service's configuration.
            </li>
            <li>
              <strong>You authorize the address</strong> below: tick what it may do, set how long, approve in Keel Wallet.
            </li>
            <li>
              <strong>The service signs with its secret.</strong> The chain accepts its actions as yours, within the permissions, until the date. Renew before then; revoke at any time from the
              list at the bottom.
            </li>
          </ol>
          <p>
            An address is public (64 hex characters) and is what goes in this page. A secret is the private half: it belongs only in the service's own configuration, and is never typed into Harbor.
          </p>
        </div>
      </Card>
    <div className="grid-2">
      <Card title="Authorize a service key">
        <form className="form" onSubmit={submit}>
          <p className="small text-2">
            The service that runs your exchange (your bridge or backend) signs with a key of its own, so it never holds this account's key and never competes with what you sign here. It creates the
            key and sends you a link that fills this form in, or its <strong>address</strong> to paste below; you approve in Keel Wallet. Never paste a secret here.
          </p>
          {asked && (
            <Notice tone="info" title={`${asked.name ?? 'A service'} asks to sign for this account.`}>
              The link you opened filled in its address and the permissions it asks for. Check them, change them if you wish, and approve in Keel Wallet. Only do this for a service you run or
              trust.
            </Notice>
          )}
          {!asked && !made && (
            <div className="row">
              <button type="button" className="btn" onClick={create}>
                Create a new key here
              </button>
              <span className="small text-2">or paste the address of a key the service already has.</span>
            </div>
          )}
          {!asked && (
            <p className="small text-2">
              <strong>Where a key created here lives.</strong> It is generated in this browser tab and held in its memory only. Harbor has no server: the secret is never stored, never sent
              anywhere, and never written to this browser's storage. It is shown to you once; closing or reloading the page discards it, and nobody, Keel included, can show it again. From then on
              the only copy is the one you put in your service. See the <a href="https://keelchain.com/policy/#harbor-keys">policy</a>.
            </p>
          )}
          {made && (
            <Notice tone="warn" title="Copy this secret into your service now. It is shown only this once.">
              <p className="small">
                It was created in your browser and is not saved or sent anywhere. Put it in the service's configuration as its Keel signing secret (for the CLI: <span className="mono">--secret</span>
                ; for the SDK: <span className="mono">Keypair.fromSecret</span>). If you lose it, create another key and revoke this one.
              </p>
              <p className="mono small" style={{ wordBreak: 'break-all' }} aria-label="Secret of the new key">
                {made.secret}
              </p>
              <div className="row">
                <button type="button" className="btn small" onClick={copySecret}>
                  {copied ? 'Copied' : 'Copy secret'}
                </button>
                <span className="small text-2">Its address is filled in below. Authorize it once the secret is stored.</span>
              </div>
            </Notice>
          )}
          {isSecret && (
            <Notice tone="bad" title="You pasted a secret, not an address.">
              Authorizing it would publish that secret on the chain for anyone to use. Clear the box and paste the key's <strong>address</strong> instead. A secret goes only into your service's
              configuration.
            </Notice>
          )}
          <Field label="The service's Keel address" hint={text !== '' && !key ? <span className="down">A Keel address is 64 hex characters.</span> : 'The public address, never the secret: both are 64 hex characters, so check which one you are pasting.'}>
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
      <Card title="If someone else runs the service">
        <div className="prose small">
          <p>
            They create the key on their side (<span className="mono">keel keygen</span> prints an address and a secret; the SDK's <span className="mono">Keypair.random()</span> does the same), keep
            the secret, and send you only the address. To spare you the copying they can send this link with their address in it; opening it fills the form above:
          </p>
          <p className="mono" style={{ wordBreak: 'break-all' }}>
            {`${window.location.origin}${window.location.pathname}?key=<their address>&scope=12&name=Bridge`}
          </p>
          <p>
            <span className="mono">scope</span> is 4 for operations, 8 for payouts, 12 for both. You still choose what to grant before you approve.
          </p>
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

// ---------------------------------------------------------------- public profile

function Profile({ h }: { h: Harbor }) {
  const act = useAction();
  const { network } = useNetwork();
  const listing = useClients().data?.find((c) => c.address === h.address);
  const saved = h.client.profile ?? null;
  const [name, setName] = useState(saved?.name ?? '');
  const [site, setSite] = useState(saved?.website ?? '');
  const [logo, setLogo] = useState(saved?.logo ?? '');
  const [copied, setCopied] = useState(false);
  const host = hostOf(site);
  const problems = { name: nameProblem(name), website: websiteProblem(host), logo: logoProblem(logo.trim()) };
  const valid = !problems.name && !problems.website && !problems.logo;
  const changed = name !== (saved?.name ?? '') || host !== (saved?.website ?? '') || logo.trim() !== (saved?.logo ?? '');
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!valid) return;
    void act.run(
      { SetClientProfile: { name, website: host, logo: logo.trim() } },
      'Set your public profile',
      `Lists this account as ${name}${host ? `, at ${host}` : ''}. Anyone can see it; you can change it at any time.`,
    );
  };
  const copy = () => void navigator.clipboard?.writeText(h.address).then(() => setCopied(true));
  const verified = saved?.website ? (listing?.website_verified ?? null) : null;
  return (
    <div className="grid-2">
      <Card title="Public profile">
        <form className="form" onSubmit={submit}>
          <p className="small text-2">
            The name your exchange is listed under on Keel. Every offer your users post carries it, so anyone browsing offers, in the explorer or through the API, can filter to yours. The chain
            puts it there from the account that vouched for the user: nobody can post an offer under your name but your own users.
          </p>
          <Field label="Name" hint={name && problems.name ? <span className="down">{problems.name}</span> : 'No other client may hold the same name, in any spelling.'}>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} maxLength={NAME_MAX} required aria-invalid={!!name && !!problems.name} placeholder="Acme Exchange" />
          </Field>
          <Field label="Website (optional)" hint={problems.website ? <span className="down">{problems.website}</span> : 'The host name only. It shows as verified once you prove it is yours.'}>
            <input className="input" value={site} onChange={(e) => setSite(e.target.value)} inputMode="url" aria-invalid={!!problems.website} placeholder="acme.example" />
          </Field>
          <Field label="Logo (optional)" hint={problems.logo ? <span className="down">{problems.logo}</span> : 'An https:// link to a square image.'}>
            <input className="input" value={logo} onChange={(e) => setLogo(e.target.value)} inputMode="url" maxLength={LOGO_MAX} aria-invalid={!!problems.logo} placeholder="https://acme.example/logo.png" />
          </Field>
          <div className="row">
            <Submit act={act} disabled={!valid || !changed}>
              {saved ? 'Save profile' : 'Publish profile'}
            </Submit>
            <span className="small text-2">{saved ? `Listed as ${saved.name}.` : 'Not listed under a name yet.'}</span>
          </div>
          <ActionResult act={act} />
        </form>
      </Card>
      <div className="stack">
        <Card title="Prove the website is yours">
          {!saved?.website ? (
            <p className="small text-2">Save a website first. Until it is proved, the directory shows it as unverified.</p>
          ) : (
            <div className="stack">
              <div className="row">
                <strong>{saved.website}</strong>
                {verified === true ? <Badge tone="good">Verified</Badge> : verified === false ? <Badge tone="warn">Not verified</Badge> : <Badge>Checking</Badge>}
              </div>
              <ol className="prose small">
                <li>
                  Create a text file whose only content is this account's address:
                  <div className="row">
                    <Hex value={h.address} />
                    <button type="button" className="btn" onClick={copy}>
                      {copied ? 'Copied' : 'Copy address'}
                    </button>
                  </div>
                </li>
                <li>
                  Serve it at <code>{proofUrl(saved.website)}</code>, over HTTPS, without a redirect.
                </li>
                <li>Wait: the file is read again every ten minutes, and the badge above turns to Verified.</li>
              </ol>
              <p className="small text-2">Only someone who controls the website can put the file there, so a verified website cannot be claimed by another client. Remove the file and the badge goes.</p>
            </div>
          )}
        </Card>
        <Card title="Where it shows">
          <p className="small text-2">
            In the <a href={explorerUrl(network.id, '/exchanges')} target="_blank" rel="noreferrer noopener">exchanges directory</a> of the explorer, on each offer of your users, and in the API:{' '}
            <code>GET /v1/clients</code> lists every exchange and <code>GET /v1/offers?client={h.address.slice(0, 8)}…</code> returns the offers under yours.
          </p>
          <KV rows={[['Offers listed under you now', formatInt(listing?.open_offers ?? 0)]]} />
        </Card>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- custody

/** What a signer's `GET /info` answers: the public half of the keys it holds. */
interface SignerInfo {
  public_key?: string | null;
  chain_code?: string | null;
  ed25519_public_key?: string | null;
  ed25519_chain_code?: string | null;
}

/** The key and chain code a network's vault takes from a signer, or null when the signer has none for it. */
function keysFor(chain: Chain, info: SignerInfo): { pk: string; cc: string } | null {
  const [pk, cc] = chain === 'Solana' ? [info.ed25519_public_key, info.ed25519_chain_code] : [info.public_key, info.chain_code];
  return pk && cc ? { pk: pk.toLowerCase(), cc: cc.toLowerCase() } : null;
}

/**
 * `RegisterCustodyVault` for the networks the client ticks. The keys are
 * read from the client's own signer (`GET <signer>/info`), so the only
 * thing typed is the signer's URL. One action, and one wallet approval,
 * for all of them.
 */
function RegisterVault({ c }: { c: Custody }) {
  const w = useWallet();
  const qc = useQueryClient();
  const taken = new Set(c.vaults.map((v) => chainOf(v.chain)));
  const free = CHAINS.filter((x) => !taken.has(x));
  const [url, setUrl] = useState('');
  const [info, setInfo] = useState<SignerInfo | null>(null);
  const [reading, setReading] = useState(false);
  const [picked, setPicked] = useState<Chain[]>(() => CHAINS.filter((x) => !c.vaults.some((v) => chainOf(v.chain) === x)));
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState<{ chain: Chain; ok: boolean; text: string }[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [manual, setManual] = useState(false);
  const [pk, setPk] = useState('');
  const [cc, setCc] = useState('');
  if (free.length === 0) return null;
  const base = url.trim().replace(/\/+$/, '');
  const urlOk = /^https?:\/\/\S+$/.test(base);
  const manualInfo: SignerInfo | null = manual && /^[0-9a-fA-F]{64,66}$/.test(pk.trim()) && /^[0-9a-fA-F]{64}$/.test(cc.trim()) ? (pk.trim().length === 64 ? { ed25519_public_key: pk.trim(), ed25519_chain_code: cc.trim() } : { public_key: pk.trim(), chain_code: cc.trim() }) : null;
  const keys = manual ? manualInfo : info;
  // Before the keys are known every network can be ticked; after, only those the signer has a key for.
  const offered = keys ? free.filter((x) => keysFor(x, keys) !== null) : free;
  const chosen = picked.filter((x) => offered.includes(x));

  const read = async () => {
    setError(null);
    setInfo(null);
    // A browser refuses a plain-HTTP request made from an HTTPS page, before it ever leaves the machine.
    if (window.location.protocol === 'https:' && base.startsWith('http://')) {
      setError('This page is served over HTTPS, and your browser will not let it read an http:// address. Put the signer behind HTTPS (a web server or a tunnel in front of its port) and give that https:// address. The observers reach it the same way.');
      return;
    }
    setReading(true);
    try {
      const res = await fetch(`${base}/info`, { headers: { accept: 'application/json' } });
      if (!res.ok) throw new Error(`the signer answered ${res.status}`);
      const got = (await res.json()) as SignerInfo;
      if (!got.public_key && !got.ed25519_public_key) throw new Error('the signer reported no keys');
      setInfo(got);
      setPicked((p) => p.filter((x) => keysFor(x, got) !== null));
    } catch (e) {
      setError(`Could not read the signer at ${base}/info: ${e instanceof Error ? e.message : String(e)}. Three things to check: the signer's port is open to the internet (not only to its own machine, and not blocked by a firewall); it is a build that answers /info (the current command-line tools); and the address is right. Or enter its keys by hand.`);
    } finally {
      setReading(false);
    }
  };
  const register = async () => {
    if (!keys || !urlOk) return;
    setBusy(true);
    setError(null);
    setLog([]);
    // One action for every network ticked: a single approval, and either
    // all of them are registered or none.
    const list = chosen.map((chain) => {
      const k = keysFor(chain, keys)!;
      return { chain, epoch: 1, public_key: k.pk, chain_code: k.cc, signer_url: base };
    });
    const names = chosen.map((x) => CHAIN_NAME[x]).join(', ');
    try {
      const r = await w.signAndSubmit(list.length === 1 ? { RegisterCustodyVault: list[0] } : { RegisterCustodyVaults: list }, {
        title: list.length === 1 ? `Register your ${names} vault` : `Register your vaults on ${list.length} networks`,
        description: `${names}. Deposit addresses for your users are derived from these keys, and your signer at ${base} signs their payouts.`,
      });
      setLog(chosen.map((chain) => ({ chain, ok: r.ok, text: r.ok ? 'registered' : (r.error?.message ?? 'refused') })));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    await qc.invalidateQueries();
    setBusy(false);
  };
  return (
    <Card title="Register vaults">
      <div className="form">
        <p className="small text-2">
          A vault is a key <strong>you</strong> hold, on a signer you run (<span className="mono">keel-tss serve</span>, or your own KMS behind the same two routes). Keel never has the key: it learns
          the public half and where to ask for signatures. Tick the networks, give the signer's address, approve.
        </p>
        <details className="small text-2">
          <summary>I don't have a signer yet</summary>
          <p>
            The signer is a small program on a server of yours that holds the vault key and signs your users' payouts after checking each one against the chain. Until it runs there is nothing to
            register. On the testnet, the quickest way is the development signer from the command-line tools (<a href="https://keelchain.com/docs/install/">install</a>):
          </p>
          <p className="mono" style={{ wordBreak: 'break-all' }}>
            keel-observer dev-signer --seed &lt;64 hex characters you generate and keep&gt; --listen 127.0.0.1:7300
          </p>
          <p>
            Put it behind HTTPS on a public name (your web server or a tunnel), and give that address below. For real funds use <span className="mono">keel-tss serve</span> with key shares, or your own
            key service answering the same two routes; see <a href="https://keelchain.com/docs/models/">the custody model</a>. If you would rather not hold keys at all, you do not need a vault: your
            users can keep their coins in Keel's network vault, with nothing to register.
          </p>
        </details>
        <fieldset className="form">
          <legend className="field-label">1. Networks to open a vault on</legend>
          <div className="row">
            {free.map((x) => {
              const can = offered.includes(x);
              return (
                <label key={x} className="small" title={can ? undefined : 'This signer holds no key for it'}>
                  <input type="checkbox" disabled={!can || busy} checked={can && picked.includes(x)} onChange={(e) => setPicked((p) => (e.target.checked ? [...p, x] : p.filter((y) => y !== x)))} /> {CHAIN_NAME[x]}
                  {!can && <span className="text-2"> (no key on this signer)</span>}
                </label>
              );
            })}
          </div>
          {taken.size > 0 && <span className="small text-2">Already registered: {CHAINS.filter((x) => taken.has(x)).map((x) => CHAIN_NAME[x]).join(', ')}.</span>}
        </fieldset>
        <Field label="2. Your signer's URL" hint="Public HTTPS address of the signer. The observers that build your payouts call it too.">
          <input className="input mono" value={url} onChange={(e) => { setUrl(e.target.value); setInfo(null); }} placeholder="https://signer.example.com" spellCheck={false} />
        </Field>
        {!manual && (
          <div className="row">
            <button type="button" className="btn" disabled={!urlOk || reading} onClick={() => void read()}>
              {reading ? 'Reading…' : info ? 'Read again' : '3. Read keys from the signer'}
            </button>
            <button type="button" className="btn small" onClick={() => setManual(true)}>
              Enter the keys by hand
            </button>
          </div>
        )}
        {manual && (
          <>
            <Field label="Public key" hint="33 bytes hex (Bitcoin, Ethereum, Tron, BNB Smart Chain) or 32 bytes hex (Solana)">
              <input className="input mono" value={pk} onChange={(e) => setPk(e.target.value)} spellCheck={false} />
            </Field>
            <Field label="Chain code" hint="32 bytes, hex">
              <input className="input mono" value={cc} onChange={(e) => setCc(e.target.value)} spellCheck={false} />
            </Field>
            <div className="row">
              <button type="button" className="btn small" onClick={() => setManual(false)}>
                Read them from the signer instead
              </button>
            </div>
          </>
        )}
        {error && <p className="small down">{error}</p>}
        {keys && (
          <div className="row">
            <button type="button" className="btn primary" disabled={busy || chosen.length === 0 || !urlOk} onClick={() => void register()}>
              {busy ? 'Waiting for the wallet…' : `Register ${chosen.length} ${chosen.length === 1 ? 'vault' : 'vaults'}`}
            </button>
            {chosen.length > 1 && <span className="small text-2">One approval in Keel Wallet for all of them.</span>}
          </div>
        )}
        {log.map((l) => (
          <p key={l.chain} className={`small ${l.ok ? 'up' : 'down'}`}>
            {CHAIN_NAME[l.chain]}: {l.text}
          </p>
        ))}
      </div>
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

/**
 * Rotate vaults to a fresh key: tick the networks, give the new signer's
 * URL, the keys are read from it, one approval for all.
 */
function Rotate({ c }: { c: Custody }) {
  const w = useWallet();
  const qc = useQueryClient();
  const pending = new Set(c.rotations.map((r) => chainOf(r.chain)));
  const active = c.vaults.filter((v) => v.status === 'Active');
  const have = CHAINS.filter((x) => active.some((v) => chainOf(v.chain) === x));
  const [picked, setPicked] = useState<Chain[]>(have.filter((x) => !pending.has(x)));
  const [url, setUrl] = useState('');
  const [info, setInfo] = useState<SignerInfo | null>(null);
  const [reading, setReading] = useState(false);
  const [at, setAt] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  if (have.length === 0) {
    return (
      <Card title="Rotate to a fresh key">
        <p className="text-2">There is no vault to rotate yet. Register one under Vaults first.</p>
      </Card>
    );
  }
  const base = url.trim().replace(/\/+$/, '');
  const urlOk = /^https?:\/\/\S+$/.test(base);
  // Why a network cannot be rotated to this signer, or null when it can.
  const blocked = (x: Chain): string | null => {
    if (pending.has(x)) return 'a rotation is already scheduled';
    const v = active.find((y) => chainOf(y.chain) === x);
    if (!info || !v) return null;
    const k = keysFor(x, info);
    if (!k) return 'no key on this signer';
    if (k.pk === v.vault.public_key.toLowerCase()) return 'this signer holds the key already in use';
    if (base === v.signer_url.replace(/\/+$/, '')) return 'the new key needs its own signer address';
    return null;
  };
  const chosen = picked.filter((x) => have.includes(x) && blocked(x) === null);
  const heightOk = at.trim() === '' || /^[0-9]+$/.test(at.trim());

  const read = async () => {
    setError(null);
    setInfo(null);
    if (window.location.protocol === 'https:' && base.startsWith('http://')) {
      setError('This page is served over HTTPS, and your browser will not let it read an http:// address. Put the new signer behind HTTPS and give that address.');
      return;
    }
    setReading(true);
    try {
      const res = await fetch(`${base}/info`, { headers: { accept: 'application/json' } });
      if (!res.ok) throw new Error(`the signer answered ${res.status}`);
      const got = (await res.json()) as SignerInfo;
      if (!got.public_key && !got.ed25519_public_key) throw new Error('the signer reported no keys');
      setInfo(got);
    } catch (e) {
      setError(`Could not read the signer at ${base}/info: ${e instanceof Error ? e.message : String(e)}. Check that its port is open to the internet, that it is a current build, and the address.`);
    } finally {
      setReading(false);
    }
  };
  const rotate = async () => {
    if (!info || !urlOk || !heightOk || chosen.length === 0) return;
    setBusy(true);
    setError(null);
    setDone(null);
    const list = chosen.map((chain) => {
      const k = keysFor(chain, info)!;
      return { chain, public_key: k.pk, chain_code: k.cc, signer_url: base, activate_at: at.trim() ? Number(at.trim()) : null };
    });
    const names = chosen.map((x) => CHAIN_NAME[x]).join(', ');
    try {
      const r = await w.signAndSubmit(list.length === 1 ? { RotateCustodyVault: list[0] } : { RotateCustodyVaults: list }, {
        title: list.length === 1 ? `Rotate your ${names} vault to a fresh key` : `Rotate your vaults on ${list.length} networks to a fresh key`,
        description: `${names}. Every user gets a new deposit address there; the old key keeps crediting and paying out until it is empty.`,
      });
      if (r.ok) setDone(`${names}: ${at.trim() ? `rotation scheduled for height ${at.trim()}` : 'rotated'}.`);
      else setError(r.error?.message ?? 'The chain refused the rotation.');
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    await qc.invalidateQueries();
    setBusy(false);
  };
  return (
    <Card title="Rotate to a fresh key">
      <div className="form">
        <p className="small text-2">
          For a suspected leak, a change of custody provider, or routine hygiene. Start a <strong>second</strong> signer holding a new key, then point the networks at it here. Every user gets a new
          deposit address on those networks; Keel accounts and balances do not change. The old key keeps crediting its addresses and paying withdrawals until it is empty and its grace is over, so
          keep the old signer running meanwhile. Nothing is ever moved from the old key to the new one.
        </p>
        <fieldset className="form">
          <legend className="field-label">1. Networks to rotate</legend>
          <div className="row">
            {have.map((x) => {
              const why = blocked(x);
              return (
                <label key={x} className="small">
                  <input type="checkbox" disabled={why !== null || busy} checked={why === null && picked.includes(x)} onChange={(e) => setPicked((p) => (e.target.checked ? [...p, x] : p.filter((y) => y !== x)))} /> {CHAIN_NAME[x]}
                  {why && <span className="text-2"> ({why})</span>}
                </label>
              );
            })}
          </div>
        </fieldset>
        <Field label="2. The new signer's URL" hint="A different address from the signer in use: both run side by side until the old key has drained.">
          <input className="input mono" value={url} onChange={(e) => { setUrl(e.target.value); setInfo(null); }} placeholder="https://signer-2.example.com" spellCheck={false} />
        </Field>
        <div className="row">
          <button type="button" className="btn" disabled={!urlOk || reading} onClick={() => void read()}>
            {reading ? 'Reading…' : info ? 'Read again' : '3. Read keys from the new signer'}
          </button>
        </div>
        <details className="small text-2">
          <summary>Schedule it for later</summary>
          <Field label="Activate at height" hint="Empty = at once. A scheduled rotation can be cancelled under Vaults until it activates.">
            <input className="input" value={at} onChange={(e) => setAt(e.target.value)} inputMode="numeric" />
          </Field>
        </details>
        {error && <p className="small down">{error}</p>}
        {info && (
          <div className="row">
            <button type="button" className="btn primary" disabled={busy || chosen.length === 0 || !urlOk || !heightOk} onClick={() => void rotate()}>
              {busy ? 'Waiting for the wallet…' : `Rotate ${chosen.length} ${chosen.length === 1 ? 'vault' : 'vaults'}`}
            </button>
            {chosen.length > 1 && <span className="small text-2">One approval in Keel Wallet for all of them.</span>}
          </div>
        )}
        {done && <p className="small up">{done}</p>}
      </div>
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

function CustodyTab({ h, tab }: { h: Harbor; tab: Exclude<Tab, 'fees' | 'profile' | 'keys'> }) {
  const custody = useCustody(h.address);
  if (custody.isError) return <ErrorState error={custody.error} what="custody vaults" />;
  if (!custody.data) {
    return (
      <Card>
        <Loading rows={5} />
      </Card>
    );
  }
  if (tab === 'rotate') return <Rotate c={custody.data} />;
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
          {tab === 'fees' ? <Fees h={h} /> : tab === 'profile' ? <Profile h={h} /> : tab === 'keys' ? <ServiceKeys h={h} /> : <CustodyTab h={h} tab={tab} />}
        </>
      ) : (
        <Card>
          <p className="text-2">Retail fees, attested users and vaults belong to a registered client. They open here, for this same account, once it is registered.</p>
        </Card>
      )}
    </>
  );
}
