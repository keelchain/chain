import { useState, type FormEvent } from 'react';
import { useBook, useUnlockDelay } from '../api/hooks';
import { ActionResult, Field, Submit, useAction } from '../components/action';
import { WALLET_URL } from '../components/Layout';
import { Amount, Card, Hex, Notice } from '../components/ui';
import { KEEL_DECIMALS, USD_DECIMALS, big, fromUnits, keel, money, shortfall, toUnits } from '../lib/amounts';
import { asksOf, quoteBuy } from '../lib/book';
import { isRunning, offeredTiers, periodLabel } from '../lib/plan';
import { useNetwork } from '../network/NetworkContext';
import { explorerUrl } from '../network/networks';
import { useHarbor, type Harbor } from './ClientGate';
import { OwedNotice, dateTime } from './shared';

function Balances({ h }: { h: Harbor }) {
  const asset = h.client.billing_asset;
  const unlocking = h.account.budget.unlocking.reduce((sum, u) => sum + big(u.amount), 0n);
  return (
    <div className="tiles four">
      <div className="tile">
        <div className="label">{asset} balance</div>
        <div className="value">
          <Amount value={h.funds.usdt} asset={asset} sym={false} />
        </div>
        <div className="hint">Pays the plan fee and usage</div>
      </div>
      <div className="tile">
        <div className="label">KEEL balance</div>
        <div className="value">
          <Amount value={h.funds.keel} asset="KEEL" sym={false} />
        </div>
        <div className="hint">Unlocked, free to lock</div>
      </div>
      <div className="tile">
        <div className="label">KEEL locked</div>
        <div className="value">
          <Amount value={h.client.locked_keel} asset="KEEL" sym={false} />
        </div>
        <div className="hint">{h.client.tier > 0 ? `Meets tier ${h.client.tier}` : 'Meets no tier'}</div>
      </div>
      <div className="tile">
        <div className="label">KEEL unlocking</div>
        <div className="value">
          <Amount value={unlocking} asset="KEEL" sym={false} />
        </div>
        <div className="hint">On its way back to the balance</div>
      </div>
    </div>
  );
}

function TopUp({ h }: { h: Harbor }) {
  const { network } = useNetwork();
  const asset = h.client.billing_asset;
  return (
    <Card title={`Add ${asset}`}>
      <div className="prose">
        <p>
          The plan and usage are paid from this account's {asset} balance on Keel. Harbor does not handle deposits; {asset} reaches the account in one of two ways:
        </p>
        <ol>
          <li>
            <strong>Deposit and convert in Keel Wallet.</strong> The wallet's Receive card gives this account a deposit address on an external chain. Send a dollar stablecoin there, then convert it to{' '}
            {asset} from the wallet's Home page. <a href={WALLET_URL}>Keel Wallet</a>
          </li>
          <li>
            <strong>Transfer on Keel.</strong> Any Keel account can send {asset} straight to this address:
            <div className="address-box">
              <Hex value={h.address} full />
            </div>
          </li>
        </ol>
        <p className="small text-2">
          Deposits in flight show on the{' '}
          <a href={explorerUrl(network.id, `/account/${h.address}`)} target="_blank" rel="noreferrer noopener">
            account's explorer page ↗
          </a>
          .
        </p>
      </div>
    </Card>
  );
}

function BuyKeel({ h }: { h: Harbor }) {
  const act = useAction();
  const asset = h.client.billing_asset;
  const [budget, setBudget] = useState('');
  const units = toUnits(budget, USD_DECIMALS);
  const tooMuch = units !== null && units > h.funds.usdt;
  const { network } = useNetwork();
  const pair = `KEEL-${asset}`;
  const book = useBook(pair);
  const asks = asksOf(book.data);
  const empty = book.isSuccess && asks.length === 0;
  const quote = units !== null && units > 0n && asks.length > 0 ? quoteBuy(asks, units) : null;
  // Four decimals is as fine as a price per KEEL needs to read.
  const price = (p: bigint) => `${Number((Number(p) / 10 ** USD_DECIMALS).toFixed(4))} ${asset}`;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (units === null || units === 0n || asks.length === 0) return;
    void act.run(
      { PlaceOrder: { pair: `KEEL-${asset}`, side: 'buy', order_type: 'market', price: null, quantity: null, quote_budget: units, client_id: null } },
      `Buy KEEL with ${budget} ${asset}`,
      'A market order on the KEEL order book; it fills at the best asks up to this budget.',
    );
  };
  return (
    <Card title="Buy KEEL">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">
          A market order on the chain's own <a href={explorerUrl(network.id, `/markets/${pair}`)}>{pair} order book</a>, paid from your {asset} balance. It buys from whoever is selling KEEL there,
          cheapest first; what it buys lands in your KEEL balance, ready to lock.
        </p>
        {book.isError && <p className="small down">The order book could not be read. You can still see it on the explorer.</p>}
        {empty && (
          <Notice tone="warn" title="Nobody is selling KEEL on the book right now.">
            A market order would buy nothing and your {asset} would stay where it is. KEEL can also be transferred to this account from another one.
          </Notice>
        )}
        {asks.length > 0 && (
          <table className="tbl" aria-label={`Sellers on ${pair}`}>
            <thead>
              <tr>
                <th>Price per KEEL</th>
                <th className="num">KEEL on offer</th>
              </tr>
            </thead>
            <tbody>
              {asks.slice(0, 5).map((a, i) => (
                <tr key={i}>
                  <td>
                    {price(a.price)}
                    {a.house ? <span className="text-2"> · liquidity pool</span> : null}
                  </td>
                  <td className="num">{keel(a.size)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <Field label={`Spend (${asset})`} hint={tooMuch ? <span className="down">More than the {money(h.funds.usdt, asset)} you hold.</span> : `You hold ${money(h.funds.usdt, asset)}.`}>
          <input className="input" value={budget} onChange={(e) => setBudget(e.target.value)} inputMode="decimal" required aria-invalid={tooMuch || (budget !== '' && units === null)} />
        </Field>
        {quote && quote.bought > 0n && (
          <p className="small" role="status">
            Buys about <strong>{keel(quote.bought)}</strong> for {money(quote.filled ? units! : quote.spent, asset)}, at {price(quote.average ?? 0n)} on average
            {quote.worst !== null && quote.worst !== quote.average ? ` (up to ${price(quote.worst)})` : ''}.
            {!quote.filled ? ` The book only has that much on offer; the other ${money(units! - quote.spent, asset)} stays in your balance.` : ''} An estimate: the book can move before the order
            lands.
          </p>
        )}
        <div className="row">
          <Submit act={act} disabled={units === null || units === 0n || tooMuch || asks.length === 0}>
            Buy at market
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function Lock({ h }: { h: Harbor }) {
  const act = useAction();
  const [text, setText] = useState('');
  const locked = big(h.client.locked_keel);
  const units = toUnits(text, KEEL_DECIMALS);
  const tooMuch = units !== null && units > h.funds.keel;
  // One button per tier the lock does not meet yet: the exact difference.
  const presets = offeredTiers(h.client)
    .map((t) => ({ tier: t.tier, need: shortfall(big(t.lock_keel), locked) }))
    .filter((p) => p.need > 0n);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (units === null || units === 0n) return;
    void act.run({ LockBudget: { amount: units } }, `Lock ${keel(units)}`, 'Locked KEEL stays yours and selects your plan tier; unlocking returns it after the unlock delay.');
  };
  return (
    <Card title="Lock KEEL">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">The lock is half of a plan: a tier can be bought, and keeps counting, only while its KEEL is locked on this account. It also gives the account its own action capacity.</p>
        <Field label="Amount (KEEL)" hint={tooMuch ? <span className="down">More than the {keel(h.funds.keel)} you hold unlocked. Buy or receive KEEL first.</span> : `You hold ${keel(h.funds.keel)} unlocked.`}>
          <input className="input" value={text} onChange={(e) => setText(e.target.value)} inputMode="decimal" required aria-invalid={tooMuch || (text !== '' && units === null)} />
        </Field>
        {presets.length > 0 && (
          <div className="row" role="group" aria-label="Amounts that reach a tier">
            {presets.map((p) => (
              <button key={p.tier} type="button" className="btn small" onClick={() => setText(fromUnits(p.need, KEEL_DECIMALS))}>
                Tier {p.tier}: {keel(p.need)}
              </button>
            ))}
          </div>
        )}
        <div className="row">
          <Submit act={act} disabled={units === null || units === 0n || tooMuch}>
            Lock
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function Unlock({ h }: { h: Harbor }) {
  const act = useAction();
  const delay = useUnlockDelay();
  const [text, setText] = useState('');
  const [sure, setSure] = useState(false);
  const c = h.client;
  const plan = c.plan;
  const locked = big(c.locked_keel);
  const units = toUnits(text, KEEL_DECIMALS);
  const tooMuch = units !== null && units > locked;
  const after = units !== null && !tooMuch ? locked - units : locked;
  const running = plan !== null && isRunning(c);
  // Only an unlock that takes a plan that counts today below its lock suspends it.
  const suspends = running && h.status === 'active' && units !== null && units > 0n && after < big(plan.lock_keel);
  const free = running ? shortfall(locked, big(plan.lock_keel)) : locked;
  const wait = delay.data !== null && delay.data !== undefined ? periodLabel(delay.data) : null;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (units === null || units === 0n) return;
    void act.run(
      { UnlockBudget: { amount: units } },
      `Unlock ${keel(units)}`,
      suspends ? `This takes the lock below your tier ${plan!.tier} plan and suspends it.` : 'The KEEL returns to your balance after the unlock delay.',
    );
  };
  return (
    <Card title="Unlock KEEL">
      <form className="form" onSubmit={submit}>
        <p className="small text-2">
          Unlocked KEEL returns to your balance {wait ? `${wait} after you sign` : 'after the unlock delay'}.{' '}
          {running ? `Your tier ${plan!.tier} plan needs ${keel(plan!.lock_keel)} locked; ${keel(free)} can be unlocked without touching it.` : 'No period is running, so nothing depends on the lock right now.'}
        </p>
        <Field label="Amount (KEEL)" hint={tooMuch ? <span className="down">More than the {keel(locked)} locked.</span> : `${keel(locked)} locked.`}>
          <input
            className="input"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setSure(false);
            }}
            inputMode="decimal"
            required
            aria-invalid={tooMuch || (text !== '' && units === null)}
          />
        </Field>
        {suspends && (
          <Notice tone="warn" title={`This suspends your tier ${plan!.tier} plan`}>
            The lock would drop to {keel(after)}, below the {keel(plan!.lock_keel)} the plan was bought with. From that block the plan stops counting, with no refund: your users pay protocol fees again and
            lose the action pool, until you lock the difference back or the period ends.
            <label className="check">
              <input type="checkbox" checked={sure} onChange={(e) => setSure(e.target.checked)} /> Unlock anyway and suspend the plan
            </label>
          </Notice>
        )}
        <div className="row">
          <Submit act={act} disabled={units === null || units === 0n || tooMuch || (suspends && !sure)}>
            Unlock
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function PendingUnlocks({ h }: { h: Harbor }) {
  const rows = h.account.budget.unlocking;
  return (
    <Card title="Pending unlocks" flush>
      {rows.length === 0 ? (
        <div className="empty">Nothing is unlocking.</div>
      ) : (
        <div className="table-wrap">
          <table className="tbl">
            <thead>
              <tr>
                <th>Back in your balance</th>
                <th className="num">Amount</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((u, i) => (
                <tr key={`${u.ready_at}:${i}`}>
                  <td>{dateTime(u.ready_at)}</td>
                  <td className="num">
                    <Amount value={u.amount} asset="KEEL" />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

export function FundsPage() {
  const h = useHarbor();
  return (
    <>
      <div className="page-head">
        <h1>Funds</h1>
        <span className="sub">What the operator account holds, and the lock.</span>
      </div>
      <OwedNotice client={h.client} funds={h.funds} link={false} />
      <Balances h={h} />
      <div className="grid-2">
        <TopUp h={h} />
        <BuyKeel h={h} />
      </div>
      <div className="grid-2">
        <Lock h={h} />
        <Unlock h={h} />
      </div>
      <PendingUnlocks h={h} />
    </>
  );
}
