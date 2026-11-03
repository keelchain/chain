/**
 * The dashboard behind the header's Connect button: any connected wallet
 * sees its account; a partner (an exchange with a custody vault, or an
 * attester) also gets its vaults and generations, rotation to a fresh
 * pool, the float swap, billing in KEEL and its retail fees. Every action
 * is signed in Keel Wallet; the page holds no key.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState, type FormEvent, type ReactNode } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import type { Receipt } from '@keelchain/sdk';
import { spendable } from '../components/Connect';
import { Amount, Badge, Card, ErrorState, Hex, KV, Loading, Tabs } from '../components/ui';
import { decimalsOf, formatInt } from '../lib/format';
import { chainName } from '../lib/links';
import { useHref } from '../network/NetworkContext';
import { useWallet, type WalletValue } from '../wallet/WalletContext';

type Tab = 'account' | 'vaults' | 'rotate' | 'swap' | 'billing' | 'fees';
const TABS: { id: Tab; label: string; partner: boolean }[] = [
  { id: 'account', label: 'Account', partner: false },
  { id: 'vaults', label: 'Vaults', partner: true },
  { id: 'rotate', label: 'Rotate', partner: true },
  { id: 'swap', label: 'Swap', partner: true },
  { id: 'billing', label: 'Billing', partner: true },
  { id: 'fees', label: 'Fees', partner: true },
];

interface Balance {
  asset: string;
  account_type: string;
  balance: string;
}
interface CustodyEntry {
  chain: string;
  gen: number;
  status: string;
  sweep: boolean;
  drain_until: number;
  swept: { asset: string; amount: string }[];
  reported_empty: number;
  vault: { epoch: number; public_key: string };
  signer_url: string;
  address: string | null;
  next_deposit_index: number;
  reserves: { asset: string; reserve: string; liabilities: string; halted: boolean }[];
}
interface Custody {
  custodian: string;
  vaults: CustodyEntry[];
  rotations: { chain: string; gen: number; activate_at: number; sweep: boolean; signer_url: string }[];
  accounts: number;
  floats: { asset: string; custody: string; deposit: string }[];
}
interface ClientRecord {
  address: string;
  attester: string | null;
  fee: { p2p_bps: number; taker_bps: number; withdraw_bps: number } | null;
  caps: { p2p_bps: number; taker_bps: number; withdraw_bps: number };
  earned: { asset: string; amount: string }[];
  usage_paid: { asset: string; amount: string }[];
  attested_accounts: number;
  usage_prices: { address_keel: string; outbound_keel: string };
}

const CHAINS = ['Bitcoin', 'Ethereum', 'Tron'] as const;
const CHAIN_CODE: Record<string, string> = { Bitcoin: 'BTC', Ethereum: 'ETH', Tron: 'TRON' };

/** Decimal text to smallest units for `asset`, or null when malformed. */
export function toUnits(text: string, decimals: number): bigint | null {
  const t = text.trim();
  if (!/^\d+(\.\d+)?$/.test(t)) return null;
  const [whole, frac = ''] = t.split('.');
  if (frac.length > decimals) return null;
  return BigInt(whole!) * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals) || '0');
}

function usePartner(w: WalletValue) {
  const addr = w.account?.address;
  const custody = useQuery({
    queryKey: ['dash-custody', addr, w.rpc],
    queryFn: () => w.rpc.custody(addr!) as Promise<Custody>,
    enabled: !!addr,
    refetchInterval: 10_000,
  });
  const client = useQuery({
    queryKey: ['dash-client', addr, w.rpc],
    queryFn: async () => {
      try {
        return (await w.rpc.client(addr!)) as ClientRecord;
      } catch (e) {
        if ((e as { status?: number }).status === 404) return null;
        throw e;
      }
    },
    enabled: !!addr,
  });
  const account = useQuery({
    queryKey: ['wallet-account', addr, w.rpc],
    queryFn: () => w.rpc.account(addr!) as Promise<{ nonce: number; balances?: Balance[] }>,
    enabled: !!addr,
    refetchInterval: 10_000,
  });
  const isPartner = (custody.data?.vaults.length ?? 0) > 0 || !!client.data;
  return { custody, client, account, isPartner };
}

function ReceiptView({ r }: { r: Receipt | null }) {
  const href = useHref();
  if (!r) return null;
  return (
    <div className={`receipt ${r.ok ? 'ok' : 'bad'}`} role="status">
      {r.ok ? <Badge tone="good">Applied</Badge> : <Badge tone="bad">Refused</Badge>}{' '}
      <Link to={href(`/tx/${r.tx_id}`)} className="mono small">
        {r.tx_id.slice(0, 16)}…
      </Link>
      {!r.ok && r.error && <span className="small text-2"> {r.error.message}</span>}
    </div>
  );
}

function useAction(w: WalletValue) {
  const qc = useQueryClient();
  const [receipt, setReceipt] = useState<Receipt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async (action: unknown, title: string) => {
    setBusy(true);
    setError(null);
    try {
      const r = await w.signAndSubmit(action, { title });
      setReceipt(r);
      await qc.invalidateQueries();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return { run, receipt, busy, error };
}

function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="small text-2">{hint}</span>}
    </label>
  );
}

// ---------------------------------------------------------------- sections

function AccountSection({ w, p }: { w: WalletValue; p: ReturnType<typeof usePartner> }) {
  const href = useHref();
  const addr = w.account!.address;
  if (p.account.isLoading) return <Loading rows={4} />;
  if (p.account.isError) return <ErrorState error={p.account.error} what="account" />;
  const balances = p.account.data?.balances ?? [];
  return (
    <Card title={<h2>Balances</h2>} actions={<Link to={href(`/account/${addr}`)} className="btn small">Explorer view</Link>} flush>
      <div className="table-wrap">
        <table className="tbl">
          <thead>
            <tr>
              <th>Asset</th>
              <th>Account</th>
              <th className="num">Balance</th>
            </tr>
          </thead>
          <tbody>
            {balances.length === 0 && (
              <tr>
                <td colSpan={3} className="text-2">
                  Nothing yet. The faucet funds testnet accounts.
                </td>
              </tr>
            )}
            {balances.map((b) => (
              <tr key={`${b.asset}:${b.account_type}`}>
                <td>{b.asset}</td>
                <td className="text-2">{b.account_type}</td>
                <td className="num">
                  <Amount value={b.balance} asset={b.asset} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="card-body small text-2">
        Address <Hex value={addr} head={10} tail={8} /> · nonce {formatInt(p.account.data?.nonce ?? 0)}
      </div>
    </Card>
  );
}

function VaultsSection({ w, p }: { w: WalletValue; p: ReturnType<typeof usePartner> }) {
  const act = useAction(w);
  if (p.custody.isLoading) return <Loading rows={6} />;
  if (p.custody.isError) return <ErrorState error={p.custody.error} what="custody vaults" />;
  const c = p.custody.data!;
  return (
    <>
      {c.rotations.length > 0 && (
        <Card title={<h2>Pending rotations</h2>}>
          {c.rotations.map((r) => (
            <div key={`${r.chain}:${r.gen}`} className="row" style={{ justifyContent: 'space-between' }}>
              <span>
                {chainName(r.chain)} → generation {r.gen} at height {formatInt(r.activate_at)} {r.sweep ? '(sweep)' : '(no sweep)'} · signer {r.signer_url}
              </span>
              <button type="button" className="btn small" disabled={act.busy} onClick={() => void act.run({ CancelCustodyRotation: { chain: r.chain === 'BTC' ? 'Bitcoin' : r.chain === 'ETH' ? 'Ethereum' : 'Tron' } }, 'Cancel the scheduled rotation')}>
                Cancel
              </button>
            </div>
          ))}
          <ReceiptView r={act.receipt} />
        </Card>
      )}
      {c.vaults.length === 0 && (
        <Card title={<h2>Custody vaults</h2>}>
          <p className="text-2">No custody vault registered. Register one with `RegisterCustodyVault` (SDK or CLI) once your signer is up; then it appears here with its generations.</p>
        </Card>
      )}
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
              {v.status === 'Draining' && <Badge tone={v.sweep ? 'accent' : 'neutral'}>{v.sweep ? 'sweeping' : 'draining by payouts'}</Badge>}
            </span>
          }
          flush
        >
          <div className="card-body">
            <KV
              rows={[
                ['Key', <Hex value={v.vault.public_key} head={12} tail={8} />],
                ['Signer', <span className="mono small">{v.signer_url}</span>],
                ['Own address (index 0)', v.address ? <span className="mono small">{v.address}</span> : '—'],
                ['Deposit indexes', formatInt(v.next_deposit_index - 1)],
                ...(v.status === 'Draining' ? [['Grace ends at height', formatInt(v.drain_until)] as [ReactNode, ReactNode], ['Observers reporting empty', formatInt(v.reported_empty)] as [ReactNode, ReactNode]] : []),
                ...(v.swept.length ? [['Swept', v.swept.map((s) => `${s.amount} ${s.asset}`).join(', ')] as [ReactNode, ReactNode]] : []),
              ]}
            />
          </div>
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
        <Card title={<h2>Floats</h2>} flush>
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

function RotateSection({ w }: { w: WalletValue }) {
  const act = useAction(w);
  const [chain, setChain] = useState<(typeof CHAINS)[number]>('Bitcoin');
  const [pk, setPk] = useState('');
  const [cc, setCc] = useState('');
  const [url, setUrl] = useState('');
  const [sweep, setSweep] = useState(false);
  const [at, setAt] = useState('');
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void act.run(
      {
        RotateCustodyVault: {
          chain,
          public_key: pk.trim(),
          chain_code: cc.trim(),
          signer_url: url.trim(),
          sweep,
          activate_at: at.trim() ? Number(at) : null,
        },
      },
      `Rotate the ${CHAIN_CODE[chain]} vault to a fresh key`,
    );
  };
  return (
    <Card title={<h2>Rotate to a fresh pool</h2>}>
      <p className="text-2 small">
        Start a second signer for a new key (its <span className="mono">keel-tss pubkey</span> prints the key and chain code), then register it here. Every account gets a new
        address on that chain; the old key keeps crediting and paying until it is empty and its grace is over. Leave sweep off when the old addresses are the problem: nothing then
        links the two pools.
      </p>
      <form className="form" onSubmit={submit}>
        <Field label="Chain">
          <select className="select" value={chain} onChange={(e) => setChain(e.target.value as (typeof CHAINS)[number])}>
            {CHAINS.map((c) => (
              <option key={c} value={c}>
                {c}
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
        <label className="check">
          <input type="checkbox" checked={sweep} onChange={(e) => setSweep(e.target.checked)} /> Sweep the old coins to the new key (links the two pools on the external chain)
        </label>
        <div className="row">
          <button type="submit" className="btn primary" disabled={act.busy || !w.account}>
            {act.busy ? 'Waiting for the wallet…' : 'Rotate'}
          </button>
          {act.error && <span className="small down">{act.error}</span>}
        </div>
        <ReceiptView r={act.receipt} />
      </form>
    </Card>
  );
}

function SwapSection({ w, p }: { w: WalletValue; p: ReturnType<typeof usePartner> }) {
  const act = useAction(w);
  const assets = p.custody.data?.floats.map((f) => f.asset) ?? [];
  const [asset, setAsset] = useState(assets[0] ?? 'BTC.BTC');
  const [amount, setAmount] = useState('');
  const [to, setTo] = useState<'deposit' | 'custody'>('deposit');
  const [behalf, setBehalf] = useState('');
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const units = toUnits(amount, decimalsOf(asset));
    if (units === null) return;
    void act.run(
      { SwapCustody: { asset, amount: units, to, on_behalf_of: behalf.trim() ? behalf.trim() : null } },
      to === 'deposit' ? 'Swap custody into the market' : 'Swap back into custody',
    );
  };
  return (
    <Card title={<h2>Float swap</h2>}>
      <p className="text-2 small">
        Move a balance between your custody and the network vault against your own floats, in one posting, with no coins moving on any external chain. Into the market (deposit) it
        trades on the book and escrow; back into custody it leaves. Sign for one of your accounts with the field below.
      </p>
      <form className="form" onSubmit={submit}>
        <Field label="Asset">
          <input className="input mono" list="dash-assets" value={asset} onChange={(e) => setAsset(e.target.value)} required />
          <datalist id="dash-assets">
            {assets.map((a) => (
              <option key={a} value={a} />
            ))}
          </datalist>
        </Field>
        <Field label="Amount">
          <input className="input" value={amount} onChange={(e) => setAmount(e.target.value)} inputMode="decimal" required />
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
          <button type="submit" className="btn primary" disabled={act.busy}>
            {act.busy ? 'Waiting for the wallet…' : 'Swap'}
          </button>
          {act.error && <span className="small down">{act.error}</span>}
        </div>
        <ReceiptView r={act.receipt} />
      </form>
    </Card>
  );
}

function BillingSection({ w, p }: { w: WalletValue; p: ReturnType<typeof usePartner> }) {
  const act = useAction(w);
  const [budget, setBudget] = useState('');
  const c = p.client.data;
  const keel = spendable(p.account.data?.balances, 'KEEL', 6);
  const buy = (e: FormEvent) => {
    e.preventDefault();
    const units = toUnits(budget, 6);
    if (units === null) return;
    void act.run(
      { PlaceOrder: { pair: 'KEEL-KUSD', side: 'buy', order_type: 'market', price: null, quantity: null, quote_budget: units, client_id: null } },
      `Buy KEEL with ${budget} KUSD`,
    );
  };
  return (
    <>
      <Card title={<h2>Billing</h2>}>
        <KV
          rows={[
            ['KEEL balance', keel],
            ['Price per custody address', c ? <Amount value={c.usage_prices.address_keel} asset="KEEL" /> : '—'],
            ['Price per custody payout or swap', c ? <Amount value={c.usage_prices.outbound_keel} asset="KEEL" /> : '—'],
            ['Usage paid', c && c.usage_paid.length ? c.usage_paid.map((u) => `${u.amount} ${u.asset}`).join(', ') : '0'],
            ['Earned (retail fees)', c && c.earned.length ? c.earned.map((u) => `${u.amount} ${u.asset}`).join(', ') : '0'],
            ['Attested accounts', c ? formatInt(c.attested_accounts) : '—'],
          ]}
        />
        <p className="text-2 small">Everything Keel charges is paid in KEEL from your deposit balance; there is no invoice. Buy KEEL on the chain's own book with KUSD.</p>
      </Card>
      <Card title={<h2>Buy KEEL</h2>}>
        <form className="form" onSubmit={buy}>
          <Field label="Spend (KUSD)">
            <input className="input" value={budget} onChange={(e) => setBudget(e.target.value)} inputMode="decimal" required />
          </Field>
          <div className="row">
            <button type="submit" className="btn primary" disabled={act.busy}>
              {act.busy ? 'Waiting for the wallet…' : 'Buy at market'}
            </button>
            {act.error && <span className="small down">{act.error}</span>}
          </div>
          <ReceiptView r={act.receipt} />
        </form>
      </Card>
    </>
  );
}

function FeesSection({ w, p }: { w: WalletValue; p: ReturnType<typeof usePartner> }) {
  const act = useAction(w);
  const c = p.client.data;
  const [p2p, setP2p] = useState(String(c?.fee?.p2p_bps ?? 0));
  const [taker, setTaker] = useState(String(c?.fee?.taker_bps ?? 0));
  const [withdraw, setWithdraw] = useState(String(c?.fee?.withdraw_bps ?? 0));
  if (!c) {
    return (
      <Card title={<h2>Retail fees</h2>}>
        <p className="text-2">Retail fees are set by attesters (clients onboarded by governance). This address is not one yet.</p>
      </Card>
    );
  }
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void act.run({ SetClientFee: { p2p_bps: Number(p2p), taker_bps: Number(taker), withdraw_bps: Number(withdraw) } }, 'Set your retail fees');
  };
  return (
    <Card title={<h2>Retail fees</h2>}>
      <p className="text-2 small">
        Basis points your users pay on top of the protocol fee, paid to you in the same posting. Caps: P2P {c.caps.p2p_bps}, taker {c.caps.taker_bps}, withdraw {c.caps.withdraw_bps}.
      </p>
      <form className="form" onSubmit={submit}>
        <Field label="P2P release (bps)">
          <input className="input" value={p2p} onChange={(e) => setP2p(e.target.value)} inputMode="numeric" pattern="[0-9]*" required />
        </Field>
        <Field label="Taker fill (bps)">
          <input className="input" value={taker} onChange={(e) => setTaker(e.target.value)} inputMode="numeric" pattern="[0-9]*" required />
        </Field>
        <Field label="Withdrawal (bps)">
          <input className="input" value={withdraw} onChange={(e) => setWithdraw(e.target.value)} inputMode="numeric" pattern="[0-9]*" required />
        </Field>
        <div className="row">
          <button type="submit" className="btn primary" disabled={act.busy}>
            {act.busy ? 'Waiting for the wallet…' : 'Save'}
          </button>
          {act.error && <span className="small down">{act.error}</span>}
        </div>
        <ReceiptView r={act.receipt} />
      </form>
    </Card>
  );
}

// ---------------------------------------------------------------- page

export function DashboardPage() {
  const w = useWallet();
  const href = useHref();
  const navigate = useNavigate();
  const { tab: tabParam } = useParams();
  const tab: Tab = (TABS.find((t) => t.id === tabParam)?.id ?? 'account') as Tab;
  const p = usePartner(w);
  if (!w.account) {
    return (
      <Card title={<h2>Dashboard</h2>}>
        <p>Connect Keel Wallet to see your account and, if you run an exchange on Keel, your vaults, rotations, floats, billing and fees.</p>
        <div className="row">
          <button type="button" className="btn primary" onClick={() => void w.connect()} disabled={w.busy}>
            {w.busy ? 'Connecting…' : 'Connect Keel Wallet'}
          </button>
          {w.detected && !w.provider && (
            <a className="btn" href="https://keelchain.com/wallet/">
              Get the wallet
            </a>
          )}
          {w.error && <span className="small down">{w.error}</span>}
        </div>
      </Card>
    );
  }
  const tabs = TABS.filter((t) => !t.partner || p.isPartner || p.custody.isLoading);
  return (
    <div className="dashboard">
      <div className="row" style={{ justifyContent: 'space-between', flexWrap: 'wrap' }}>
        <h1 style={{ margin: 0 }}>Dashboard</h1>
        <span className="small text-2">
          <Hex value={w.account.address} head={8} tail={6} /> on {w.account.network}
          {p.isPartner ? ' · partner' : ''}
        </span>
      </div>
      <Tabs tabs={tabs.map((t) => ({ id: t.id, label: t.label }))} value={tab} onChange={(t) => navigate(href(t === 'account' ? '/dashboard' : `/dashboard/${t}`))} />
      {tab === 'account' && <AccountSection w={w} p={p} />}
      {tab === 'vaults' && <VaultsSection w={w} p={p} />}
      {tab === 'rotate' && <RotateSection w={w} />}
      {tab === 'swap' && <SwapSection w={w} p={p} />}
      {tab === 'billing' && <BillingSection w={w} p={p} />}
      {tab === 'fees' && <FeesSection w={w} p={p} />}
    </div>
  );
}
