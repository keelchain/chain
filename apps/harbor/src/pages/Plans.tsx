import { useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { ActionResult, Submit, useAction, type ActionRun } from '../components/action';
import { TierCard } from '../components/TierCard';
import { Badge, Card, KV } from '../components/ui';
import { keel, money } from '../lib/amounts';
import { isRunning, offeredTiers, periodLabel, tierOffer, type TierOffer } from '../lib/plan';
import { useHref } from '../network/NetworkContext';
import { useHarbor, type Harbor } from './ClientGate';
import { OwedNotice, StatusBadge, day } from './shared';

/** What signing will do, shown before the wallet is asked. */
function Checkout({ h, offer, act, onClose }: { h: Harbor; offer: TierOffer; act: ActionRun; onClose: () => void }) {
  const c = h.client;
  const asset = c.billing_asset;
  const n = offer.tier.tier;
  const upgrade = offer.kind === 'upgrade';
  const ends = h.nowSecs + c.plan_period_secs;
  const rows: [string, string][] = [[`Tier ${n} fee`, money(offer.tier.fee_usd_micro, asset)]];
  if (upgrade) rows.push([`Credit for what tier ${c.plan!.tier} has left (estimate)`, `− ${money(offer.credit, asset)}`]);
  if (offer.owed > 0n) rows.push(['Usage owed, settled first', money(offer.owed, asset)]);
  rows.push(['Taken from your balance now', money(offer.charge, asset)]);
  rows.push(['Balance after', money(h.funds.usdt - offer.charge, asset)]);
  rows.push(['New period', `Starts in the block that includes your action, ends about ${day(ends)}`]);
  rows.push(['Renewal', `Automatic at tier ${n}; change it below at any time`]);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void act.run({ BuyPlan: { tier: n } }, upgrade ? `Upgrade to tier ${n}` : `Buy the tier ${n} plan`, `Pays about ${money(offer.charge, asset)} from your balance and starts a ${periodLabel(c.plan_period_secs)} period now.`);
  };
  return (
    <Card title={<h2>{upgrade ? `Upgrade to tier ${n}` : `Buy tier ${n}`}</h2>} actions={<button type="button" className="btn small" onClick={onClose}>Close</button>} flush>
      <KV rows={rows} />
      <form className="card-body stack-s" onSubmit={submit}>
        {upgrade && (
          <p className="small text-2">
            The credit is the old fee times the share of the period left, or the share of its included usage left when that is smaller. It shrinks as the period runs, and the chain works it out in
            the block that includes your action, so the amount taken can differ slightly from this estimate.
          </p>
        )}
        <div className="row">
          <Submit act={act} disabled={!offer.canBuy}>
            {`Sign and pay ${money(offer.charge, asset)}`}
          </Submit>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

function TierState({ h, offer, onPick }: { h: Harbor; offer: TierOffer; onPick: () => void }) {
  const href = useHref();
  const asset = h.client.billing_asset;
  const n = offer.tier.tier;
  if (offer.kind === 'current') {
    return <p className="small text-2">{h.status === 'suspended' ? 'Your plan, suspended: lock back what it was bought with to restore it.' : `Your plan until ${day(h.client.plan!.ends_at)}.`}</p>;
  }
  if (offer.kind === 'lower') {
    return <p className="small text-2">Below your running plan. Choose it under Renewal to move down when this period ends.</p>;
  }
  const verb = offer.kind === 'upgrade' ? 'Upgrade' : 'Buy';
  if (offer.canBuy) {
    return (
      <>
        <p className="small text-2">
          {offer.kind === 'upgrade' ? `About ${money(offer.charge, asset)} after credit for the period you have left.` : `Takes ${money(offer.charge, asset)} now${offer.owed > 0n ? ', including what is owed' : ''}.`}
        </p>
        <button type="button" className="btn primary" onClick={onPick}>
          {verb === 'Upgrade' ? `Upgrade to tier ${n}` : `Buy tier ${n}`}
        </button>
      </>
    );
  }
  if (!h.client.attester && offer.needLock === 0n && offer.needUsdt === 0n) {
    return <p className="small text-2">The lock and the balance are in place. You can {verb.toLowerCase()} it once this account is registered as a client.</p>;
  }
  return (
    <>
      <ul className="needs">
        {offer.needLock > 0n && <li>Needs {keel(offer.needLock)} more locked</li>}
        {offer.needUsdt > 0n && (
          <li>
            Needs {money(offer.needUsdt, asset)} more{offer.kind === 'upgrade' ? ' (after credit)' : ''}
          </li>
        )}
      </ul>
      <Link className="btn" to={href('/funds')}>
        Go to Funds
      </Link>
    </>
  );
}

function Renewal({ h }: { h: Harbor }) {
  const act = useAction();
  const c = h.client;
  const plan = c.plan!;
  const tiers = offeredTiers(c);
  const [choice, setChoice] = useState(plan.renew_tier);
  const asset = c.billing_asset;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    void act.run({ SetPlanRenewal: { tier: choice } }, choice === 0 ? 'Stop renewing the plan' : `Renew the plan at tier ${choice}`);
  };
  return (
    <Card title="Renewal">
      <form className="stack-s" onSubmit={submit}>
        <p className="text-2 small">
          {isRunning(c)
            ? `When this period ends on ${day(plan.ends_at)}, the chain starts the next one in the same block and takes that tier's fee.`
            : 'The period has ended. The chain starts the next one in the first block in which the lock and the balance allow it.'}{' '}
          If the lock no longer meets the tier you chose, the plan renews at the highest tier it does meet. If the balance does not cover the fee, there is no plan until it does.
        </p>
        <fieldset className="choices">
          <legend className="field-label">Renew at</legend>
          {tiers.map((t) => (
            <label key={t.tier} className="choice">
              <input type="radio" name="renew" checked={choice === t.tier} onChange={() => setChoice(t.tier)} />
              <span>
                <strong>Tier {t.tier}</strong>
                {t.tier === plan.tier ? ' (keep)' : t.tier < plan.tier ? ' (move down)' : ' (move up)'}
                <span className="text-2">
                  {' '}
                  · {money(t.fee_usd_micro, asset)} · {keel(t.lock_keel)} locked
                </span>
              </span>
            </label>
          ))}
          <label className="choice">
            <input type="radio" name="renew" checked={choice === 0} onChange={() => setChoice(0)} />
            <span>
              <strong>Do not renew</strong>
              <span className="text-2"> · the plan ends with the period and your users pay protocol fees after it</span>
            </span>
          </label>
        </fieldset>
        <div className="row">
          <Submit act={act} disabled={choice === plan.renew_tier}>
            Save renewal
          </Submit>
          <span className="small text-2">Now: {plan.renew_tier === 0 ? 'does not renew' : `renews at tier ${plan.renew_tier}`}</span>
        </div>
        <ActionResult act={act} />
      </form>
    </Card>
  );
}

export function PlansPage() {
  const h = useHarbor();
  const c = h.client;
  const [picked, setPicked] = useState<number | null>(null);
  // One run for the purchase, kept here so its receipt outlives the checkout
  // (which closes once the bought tier becomes the current one).
  const buy = useAction();
  const offers = offeredTiers(c).map((t) => tierOffer(c, h.funds, t, h.nowSecs));
  const pickedOffer = offers.find((o) => o.tier.tier === picked && (o.kind === 'buy' || o.kind === 'upgrade'));
  return (
    <>
      <div className="page-head">
        <h1>Plans</h1>
        <span className="sub">
          A period is {periodLabel(c.plan_period_secs)}. You have {money(h.funds.usdt, c.billing_asset)} and {keel(c.locked_keel)} locked.
        </span>
        <span className="spacer" />
        <StatusBadge status={h.status} />
      </div>
      <OwedNotice client={c} funds={h.funds} />
      {offers.length === 0 ? (
        <Card>
          <p className="text-2">No plan tier is switched on on this network yet.</p>
        </Card>
      ) : (
        <div className="plan-grid">
          {offers.map((o) => (
            <TierCard
              key={o.tier.tier}
              tier={o.tier}
              asset={c.billing_asset}
              periodSecs={c.plan_period_secs}
              perUserPerDay={c.user_actions_per_user_per_day}
              highlight={o.kind === 'current'}
              badge={o.kind === 'current' ? <Badge tone={h.status === 'suspended' ? 'bad' : 'accent'}>{h.status === 'suspended' ? 'Suspended' : 'Current'}</Badge> : undefined}
            >
              <TierState h={h} offer={o} onPick={() => setPicked(o.tier.tier)} />
            </TierCard>
          ))}
        </div>
      )}
      {pickedOffer ? (
        <Checkout key={pickedOffer.tier.tier} h={h} offer={pickedOffer} act={buy} onClose={() => setPicked(null)} />
      ) : (
        (buy.receipt || buy.error) && (
          <Card title="Last purchase">
            <ActionResult act={buy} />
          </Card>
        )
      )}
      {c.plan && <Renewal h={h} />}
    </>
  );
}
