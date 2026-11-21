import { Link } from 'react-router-dom';
import { MeterRow, type MeterTone } from '../components/Meter';
import { Amount, Card } from '../components/ui';
import { big, keel, money, ratio, shortfall } from '../lib/amounts';
import { formatInt } from '../lib/format';
import { daysLeft, nextStep, offeredTiers, stepText, tierOf } from '../lib/plan';
import { useHref } from '../network/NetworkContext';
import { useHarbor, type Harbor } from './ClientGate';
import { OwedNotice, StatusBadge, day } from './shared';

function NextStepCard({ h }: { h: Harbor }) {
  const href = useHref();
  const step = nextStep(h.client, h.funds, h.nowSecs);
  const text = stepText(step, h.client);
  const quiet = text.page === null;
  return (
    <section className={`next-step${quiet ? ' quiet' : ''}`} aria-labelledby="next-h">
      <div>
        <div className="eyebrow" id="next-h">
          Next step
        </div>
        <div className="next-title">{text.title}</div>
        <p className="text-2">{text.detail}</p>
      </div>
      {text.page && (
        <Link className="btn primary large" to={href(`/${text.page}`)}>
          {text.title}
        </Link>
      )}
    </section>
  );
}

function PlanSummary({ h }: { h: Harbor }) {
  const { client: c, status } = h;
  const plan = c.plan;
  const asset = c.billing_asset;
  const left = plan ? daysLeft(plan, h.nowSecs) : 0;
  const running = status === 'active' || status === 'suspended';
  return (
    <div className="tiles">
      <div className="tile">
        <div className="label">Plan</div>
        <div className="value">
          {plan ? `Tier ${plan.tier}` : 'None'} <StatusBadge status={status} />
        </div>
        <div className="hint">
          {status === 'suspended'
            ? 'The lock is below what the plan was bought with'
            : status === 'expired' && plan
              ? `Ended ${day(plan.ends_at)}`
              : status === 'none'
                ? 'Your users pay protocol fees'
                : `Started ${day(plan!.started_at)}`}
        </div>
      </div>
      <div className="tile">
        <div className="label">Days left</div>
        <div className="value">{running ? formatInt(left) : '—'}</div>
        <div className="hint">{running && plan ? `Ends ${day(plan.ends_at)}` : 'No period is running'}</div>
      </div>
      <div className="tile">
        <div className="label">Renewal</div>
        <div className="value">{!plan ? '—' : plan.renew_tier === 0 ? 'Off' : `Tier ${plan.renew_tier}`}</div>
        <div className="hint">
          {!plan
            ? 'Set once you have a plan'
            : plan.renew_tier === 0
              ? 'The plan ends with this period'
              : running
                ? `On ${day(plan.ends_at)}, for ${money(tierOf(c, plan.renew_tier)?.fee_usd_micro ?? '0', asset)}`
                : 'In the first block the account can pay'}
        </div>
      </div>
      <div className="tile">
        <div className="label">{asset} balance</div>
        <div className="value">
          <Amount value={h.funds.usdt} asset={asset} sym={false} />
        </div>
        <div className="hint">Pays the plan and usage</div>
      </div>
      <div className="tile">
        <div className="label">KEEL locked</div>
        <div className="value">
          <Amount value={c.locked_keel} asset="KEEL" sym={false} />
        </div>
        <div className="hint">{c.tier > 0 ? `Meets tier ${c.tier}` : 'Meets no tier'}</div>
      </div>
      <div className="tile">
        <div className="label">Usage owed</div>
        <div className="value">
          <Amount value={c.usage_owed} asset={asset} sym={false} />
        </div>
        <div className="hint">{big(c.usage_owed) > 0n ? 'Blocks your users; see above' : 'Nothing owed'}</div>
      </div>
    </div>
  );
}

function Meters({ h }: { h: Harbor }) {
  const { client: c, status } = h;
  const plan = c.plan;
  const asset = c.billing_asset;
  const running = status === 'active' || status === 'suspended';
  const locked = big(c.locked_keel);
  // The lock is measured against the running plan, else against the tier a renewal or a first purchase aims at.
  const target = running && plan ? { tier: plan.tier, lock: big(plan.lock_keel) } : (() => {
    const t = (plan && plan.renew_tier > 0 ? tierOf(c, plan.renew_tier) : undefined) ?? offeredTiers(c)[0];
    return t ? { tier: t.tier, lock: big(t.lock_keel) } : null;
  })();
  const lockShort = target ? shortfall(target.lock, locked) : 0n;

  const allowance = plan ? big(plan.allowance_usd_micro) : 0n;
  const allowanceLeft = plan ? big(plan.allowance_left_usd_micro) : 0n;
  const allowanceRatio = ratio(allowanceLeft, allowance);
  const allowanceTone: MeterTone = allowanceLeft === 0n ? 'bad' : allowanceRatio < 0.2 ? 'warn' : 'ok';

  const used = plan?.user_actions_used_today ?? 0;
  const pool = plan?.user_actions_per_day ?? 0;
  const usedRatio = pool > 0 ? Math.min(1, used / pool) : 0;
  const usedTone: MeterTone = pool > 0 && used >= pool ? 'bad' : usedRatio >= 0.8 ? 'warn' : 'ok';

  return (
    <Card title="This period">
      <div className="meter-list">
        {running && plan ? (
          <>
            <MeterRow
              label="Included usage left"
              ratio={allowanceRatio}
              tone={allowanceTone}
              state={allowanceLeft === 0n ? 'Used up' : 'Running low'}
              figure={
                <>
                  {money(allowanceLeft, asset)} of {money(allowance, asset)}
                </>
              }
              note={allowanceLeft === 0n ? `Usage is now charged to your ${asset} balance.` : `Usage draws on this before your ${asset} balance.`}
            />
            <MeterRow
              label="User actions used today"
              ratio={usedRatio}
              tone={usedTone}
              state={used >= pool ? 'Pool used up' : 'Nearly used up'}
              figure={
                <>
                  {formatInt(used)} of {formatInt(pool)}
                </>
              }
              note={
                used >= pool && pool > 0
                  ? 'Until tomorrow (UTC) your users need action capacity of their own.'
                  : `Resets every day (UTC); one user can take up to ${formatInt(c.user_actions_per_user_per_day)}.`
              }
            />
          </>
        ) : (
          <p className="text-2">No period is running, so there is no included usage and no action pool for your users.</p>
        )}
        {target && (
          <MeterRow
            label={`KEEL locked for tier ${target.tier}`}
            ratio={ratio(locked, target.lock)}
            tone={lockShort > 0n ? (status === 'suspended' ? 'bad' : 'warn') : 'ok'}
            state={status === 'suspended' ? 'Plan suspended' : 'Below the tier'}
            figure={
              <>
                {keel(locked)} of {keel(target.lock)}
              </>
            }
            note={lockShort > 0n ? `${keel(lockShort)} more to lock; you hold ${keel(h.funds.keel)} unlocked.` : 'The lock meets the tier. It stays yours and returns when you unlock it.'}
          />
        )}
      </div>
    </Card>
  );
}

export function OverviewPage() {
  const h = useHarbor();
  return (
    <>
      <div className="page-head">
        <h1>Overview</h1>
        <span className="sub">
          {h.client.attested_accounts === 1 ? '1 attested account' : `${formatInt(h.client.attested_accounts)} attested accounts`}
        </span>
      </div>
      <OwedNotice client={h.client} funds={h.funds} />
      <NextStepCard h={h} />
      <PlanSummary h={h} />
      <Meters h={h} />
    </>
  );
}
