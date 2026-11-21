import { usePublicTerms } from '../api/hooks';
import { DOCS_URL, WALLET_URL } from '../components/Layout';
import { TierCard } from '../components/TierCard';
import { Card, ErrorState, Loading } from '../components/ui';
import { money } from '../lib/amounts';
import { offeredTiers, periodLabel } from '../lib/plan';
import { useNetwork } from '../network/NetworkContext';
import { explorerUrl } from '../network/networks';
import { useWallet } from '../wallet/WalletContext';

export function ConnectButton({ label = 'Connect Keel Wallet' }: { label?: string }) {
  const w = useWallet();
  return (
    <div className="row">
      <button type="button" className="btn primary large" onClick={() => void w.connect()} disabled={w.busy}>
        {w.busy ? 'Connecting…' : label}
      </button>
      {w.detected && !w.provider && (
        <a className="btn large" href={WALLET_URL}>
          Get the wallet
        </a>
      )}
      {w.error && (
        <span className="small down" role="alert">
          {w.error}
        </span>
      )}
    </div>
  );
}

export function LandingPage() {
  const terms = usePublicTerms();
  const { network } = useNetwork();
  const t = terms.data;
  return (
    <div className="stack">
      <section className="hero">
        <h1>The client account for Keel</h1>
        <p className="lead">
          Keel Harbor is where an exchange that runs on Keel buys its membership plan, locks its KEEL, watches its usage and sets the fees it charges its own users. There is no sign-up and no
          server: every number on these pages is read from the chain, and every change is one action you sign in Keel Wallet.
        </p>
        <ConnectButton />
        <p className="small text-2">Connect the operator account that governance registered as your attester. Harbor never sees a key.</p>
      </section>

      <section aria-labelledby="plans-h" className="stack">
        <div className="page-head">
          <h2 id="plans-h">Plans</h2>
          <span className="sub">
            Live from {network.name}; governance sets them.{t ? ` A period is ${periodLabel(t.plan_period_secs)}.` : ''}
          </span>
        </div>
        {terms.isError ? (
          <ErrorState error={terms.error} what="plans" />
        ) : !t ? (
          <Card>
            <Loading rows={4} />
          </Card>
        ) : offeredTiers(t).length === 0 ? (
          <Card>
            <p className="text-2">No plan tier is switched on on {network.name} yet.</p>
          </Card>
        ) : (
          <div className="plan-grid">
            {offeredTiers(t).map((tier) => (
              <TierCard key={tier.tier} tier={tier} asset={t.billing_asset} periodSecs={t.plan_period_secs} perUserPerDay={t.user_actions_per_user_per_day} />
            ))}
          </div>
        )}
      </section>

      <div className="grid-3">
        <Card title="A fee and a lock">
          <p className="text-2">
            A plan is both: the tier's KEEL kept locked on your account, and its fee each period{t ? ` in ${t.billing_asset}` : ''}. The lock stays yours and comes back when you unlock it. The
            plan renews by itself.
          </p>
        </Card>
        <Card title="Your users pay you, not the protocol">
          <p className="text-2">
            While a plan runs, the accounts you attested pay no protocol fee and need no KEEL of their own: their actions come out of your daily pool, and you charge them the retail fee you choose.
          </p>
        </Card>
        <Card title="Usage, allowance first">
          {t ? (
            <p className="text-2">
              You pay a price per flow of your users: {money(t.usage_prices.address_usd_micro, t.billing_asset)} per deposit address, {money(t.usage_prices.outbound_usd_micro, t.billing_asset)} per
              payout, {money(t.usage_prices.fill_usd_micro, t.billing_asset)} per filled order, {money(t.usage_prices.release_usd_micro, t.billing_asset)} per escrow release and{' '}
              {money(t.usage_prices.ruling_usd_micro, t.billing_asset)} per dispute ruling. The plan's included usage pays first.
            </p>
          ) : (
            <p className="text-2">You pay a price per flow of your users; the plan's included usage pays first.</p>
          )}
        </Card>
      </div>

      <Card title="Not a client yet?">
        <p className="text-2">
          A client is an account registered as an attester by a governance proposal. The <a href={DOCS_URL}>client documentation</a> covers onboarding, the billing model and the SDK. If you are
          here for your own balances, they are in <a href={WALLET_URL}>Keel Wallet</a> and in the <a href={explorerUrl(network.id)}>explorer</a>.
        </p>
      </Card>
    </div>
  );
}
