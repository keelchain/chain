import type { ReactNode } from 'react';
import type { Tier } from '../api/types';
import { formatAmount, formatInt } from '../lib/format';
import { KEEL_DECIMALS, USD_DECIMALS } from '../lib/amounts';
import { periodLabel } from '../lib/plan';

/** One tier as a plan card: the price, then what it needs and what it includes. */
export function TierCard({
  tier,
  asset,
  periodSecs,
  perUserPerDay,
  badge,
  highlight = false,
  children,
}: {
  tier: Tier;
  asset: string;
  periodSecs: number;
  perUserPerDay: number;
  badge?: ReactNode;
  highlight?: boolean;
  /** The client's state on this tier and its button. */
  children?: ReactNode;
}) {
  return (
    <section className={`plan-card${highlight ? ' current' : ''}`} aria-label={`Tier ${tier.tier}`}>
      <div className="plan-head">
        <h3>Tier {tier.tier}</h3>
        {badge}
      </div>
      <div className="plan-price">
        <span className="figure">{formatAmount(tier.fee_usd_micro, USD_DECIMALS)}</span>
        <span className="unit">
          {asset} per {periodLabel(periodSecs)}
        </span>
      </div>
      <dl className="plan-terms">
        <div>
          <dt>KEEL kept locked</dt>
          <dd>{formatAmount(tier.lock_keel, KEEL_DECIMALS)}</dd>
        </div>
        <div>
          <dt>Usage included</dt>
          <dd>
            {formatAmount(tier.allowance_usd_micro, USD_DECIMALS)} {asset}
          </dd>
        </div>
        <div>
          <dt>Actions a day for your users</dt>
          <dd>{formatInt(tier.user_actions_per_day)}</dd>
        </div>
        <div>
          <dt>Per user, per day</dt>
          <dd>up to {formatInt(perUserPerDay)}</dd>
        </div>
      </dl>
      {children && <div className="plan-foot">{children}</div>}
    </section>
  );
}
