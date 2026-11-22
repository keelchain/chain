/**
 * Deterministic in-browser data for design work and tests: one client in a
 * chosen state. `VITE_API_MOCK=1` serves it (pick the state with
 * `?demo=none|active|suspended|owed|expired|visitor`); tests build records
 * with the helpers directly.
 */
import type { Book, AccountRecord, ClientRecord, Custody, HarborApi, HistoryPage, Plan, Tier } from './types';

export const MOCK_NOW_SECS = 1_795_000_000;
export const MOCK_ADDRESS = 'c1'.repeat(32);
const KEEL = 1_000_000n;
const USD = 1_000_000n;
const DAY = 86_400;

export const MOCK_TIERS: Tier[] = [
  { tier: 1, lock_keel: String(100_000n * KEEL), fee_usd_micro: String(300n * USD), allowance_usd_micro: String(300n * USD), user_actions_per_day: 10_000 },
  { tier: 2, lock_keel: String(500_000n * KEEL), fee_usd_micro: String(1_000n * USD), allowance_usd_micro: String(1_200n * USD), user_actions_per_day: 50_000 },
  { tier: 3, lock_keel: String(2_000_000n * KEEL), fee_usd_micro: String(3_000n * USD), allowance_usd_micro: String(4_000n * USD), user_actions_per_day: 250_000 },
];

/** A plan at `tier` that started `ageDays` ago, with overrides. */
export function mockPlan(tier: number, ageDays: number, over: Partial<Plan> = {}, nowSecs: number = MOCK_NOW_SECS): Plan {
  const t = MOCK_TIERS[tier - 1]!;
  const started = nowSecs - ageDays * DAY;
  return {
    tier,
    started_at: started,
    ends_at: started + 30 * DAY,
    active: true,
    suspended: false,
    fee_usd_micro: t.fee_usd_micro,
    allowance_usd_micro: t.allowance_usd_micro,
    allowance_left_usd_micro: t.allowance_usd_micro,
    lock_keel: t.lock_keel,
    renew_tier: tier,
    user_actions_per_day: t.user_actions_per_day,
    user_actions_used_today: 0,
    ...over,
  };
}

/** An attester with nothing locked and no plan, with overrides. */
export function mockClient(over: Partial<ClientRecord> = {}): ClientRecord {
  return {
    address: MOCK_ADDRESS,
    attester: true,
    fee: null,
    caps: { p2p_bps: 200, taker_bps: 100, withdraw_bps: 100 },
    earned: [],
    usage_paid: [],
    usage_owed: '0',
    attested_accounts: 0,
    protocol_fee_waived: true,
    usage_prices: { address_usd_micro: '50000', outbound_usd_micro: '250000', swap_usd_micro: '0', fill_usd_micro: '20000', release_usd_micro: '100000', ruling_usd_micro: '5000000' },
    billing_asset: 'USDT',
    locked_keel: '0',
    tier: 0,
    plan: null,
    user_actions_per_user_per_day: 100,
    plan_period_secs: 30 * DAY,
    tiers: MOCK_TIERS,
    ...over,
  };
}

export function mockAccount(over: { usdt?: bigint; keel?: bigint; locked?: bigint; unlocking?: { ready_at: number; amount: string }[]; custody?: bigint; held?: bigint } = {}): AccountRecord {
  return {
    address: MOCK_ADDRESS,
    nonce: 7,
    balances: [
      { asset: 'USDT', account_type: 'deposit', balance: String(over.usdt ?? 0n) },
      { asset: 'KEEL', account_type: 'deposit', balance: String(over.keel ?? 0n) },
      { asset: 'USDT', account_type: 'custody', balance: String(over.custody ?? 0n) },
      { asset: 'TRON.USDT', account_type: 'custody_hold', balance: String(over.held ?? 0n) },
    ],
    budget: { locked: String(over.locked ?? 0n), unlocking: over.unlocking ?? [] },
    sessions: [],
  };
}

export type Scenario = 'none' | 'active' | 'suspended' | 'owed' | 'expired' | 'visitor';
export const SCENARIOS: Scenario[] = ['none', 'active', 'suspended', 'owed', 'expired', 'visitor'];

export function scenarioFromLocation(): Scenario {
  try {
    const s = new URLSearchParams(window.location.search).get('demo') as Scenario | null;
    if (s && SCENARIOS.includes(s)) return s;
  } catch {
    // no window
  }
  return 'active';
}

/** The client and account records of one scenario at `nowSecs`. */
export function scenario(s: Scenario, nowSecs: number = MOCK_NOW_SECS): { client: ClientRecord; account: AccountRecord } {
  const paid = [{ asset: 'USDT', amount: String(2_340n * USD) }];
  const earned = [
    { asset: 'USDT', amount: String(812n * USD) },
    { asset: 'BTC.BTC', amount: '1250000' },
  ];
  const fee = { p2p_bps: 50, taker_bps: 10, withdraw_bps: 0 };
  switch (s) {
    case 'visitor':
      return { client: mockClient({ attester: false }), account: mockAccount({ usdt: 25n * USD, keel: 40n * KEEL }) };
    case 'none':
      return { client: mockClient(), account: mockAccount({ usdt: 120n * USD, keel: 60_000n * KEEL }) };
    case 'active':
      return {
        client: mockClient({
          fee,
          earned,
          usage_paid: paid,
          attested_accounts: 1_284,
          locked_keel: String(500_000n * KEEL),
          tier: 2,
          plan: mockPlan(2, 12, { allowance_left_usd_micro: String(742n * USD), user_actions_used_today: 18_650 }, nowSecs),
        }),
        account: mockAccount({ usdt: 1_840n * USD, keel: 25_000n * KEEL, locked: 500_000n * KEEL, custody: 900n * USD, held: 40n * USD }),
      };
    case 'suspended':
      return {
        client: mockClient({
          fee,
          earned,
          usage_paid: paid,
          attested_accounts: 310,
          locked_keel: String(60_000n * KEEL),
          tier: 0,
          plan: mockPlan(1, 9, { active: false, suspended: true, allowance_left_usd_micro: String(190n * USD) }, nowSecs),
        }),
        account: mockAccount({ usdt: 410n * USD, keel: 5_000n * KEEL, locked: 60_000n * KEEL, unlocking: [{ ready_at: (nowSecs + 2 * DAY) * 1000, amount: String(40_000n * KEEL) }] }),
      };
    case 'owed':
      return {
        client: mockClient({
          fee,
          earned,
          usage_paid: paid,
          usage_owed: String(37n * USD + 500_000n),
          attested_accounts: 96,
          locked_keel: String(100_000n * KEEL),
          tier: 1,
          plan: mockPlan(1, 21, { allowance_left_usd_micro: '0', user_actions_used_today: 4_100 }, nowSecs),
        }),
        account: mockAccount({ usdt: 0n, keel: 1_200n * KEEL, locked: 100_000n * KEEL }),
      };
    case 'expired':
      return {
        client: mockClient({
          fee,
          usage_paid: paid,
          attested_accounts: 96,
          locked_keel: String(100_000n * KEEL),
          tier: 1,
          plan: mockPlan(1, 31, { active: false, allowance_left_usd_micro: String(12n * USD) }, nowSecs),
        }),
        account: mockAccount({ usdt: 80n * USD, keel: 0n, locked: 100_000n * KEEL }),
      };
  }
}

export class MockHarborApi implements HarborApi {
  private readonly data: { client: ClientRecord; account: AccountRecord };
  private readonly nowSecs: number;

  /** `nowSecs` anchors the plan dates; the app's clock by default. */
  constructor(s: Scenario | { client: ClientRecord; account: AccountRecord }, nowSecs: number = Math.floor(Date.now() / 1000)) {
    this.nowSecs = nowSecs;
    this.data = typeof s === 'string' ? scenario(s, nowSecs) : s;
  }

  client(addr: string): Promise<ClientRecord> {
    // Any other address reads as a non-client; the tiers and prices are the same for everyone.
    return Promise.resolve(addr === this.data.client.address ? this.data.client : mockClient({ address: addr, attester: false }));
  }
  account(): Promise<AccountRecord> {
    return Promise.resolve(this.data.account);
  }
  book(pair: string): Promise<Book> {
    const level = (price: number, size: number) => ({ price: String(Math.round(price * 1e6)), size: String(BigInt(size) * KEEL), orders: 1 });
    return Promise.resolve({ symbol: pair, asks: [level(0.412, 18_000), level(0.415, 42_000), level(0.42, 150_000), level(0.431, 400_000)], bids: [level(0.405, 25_000)], house: null });
  }
  custody(addr: string): Promise<Custody> {
    return Promise.resolve({ custodian: addr, vaults: [], rotations: [], accounts: 0, floats: [] });
  }
  assets(): Promise<{ asset: string; decimals: number }[]> {
    return Promise.resolve([
      { asset: 'KEEL', decimals: 6 },
      { asset: 'USDT', decimals: 6 },
      { asset: 'BTC.BTC', decimals: 8 },
    ]);
  }
  unlockDelaySecs(): Promise<number | null> {
    return Promise.resolve(3 * DAY);
  }
  history(addr: string): Promise<HistoryPage> {
    const plan = this.data.client.plan;
    if (!plan) return Promise.resolve({ txs: [] });
    const at = (hoursAgo: number) => (this.nowSecs - hoursAgo * 3600) * 1000;
    const tx = (n: number, hoursAgo: number, signer: string, kind: string, events: HistoryPage['txs'][number]['events']) => ({
      tx_id: n.toString(16).padStart(2, '0').repeat(32),
      height: 90_000 - n * 40,
      timestamp: at(hoursAgo),
      signer,
      kind,
      ok: true,
      events,
    });
    const user = 'e7'.repeat(32);
    return Promise.resolve({
      txs: [
        tx(1, 2, user, 'PlaceOrder', [{ type: 'UsageCovered', client: addr, amount: '20000', kind: 'fill' }]),
        tx(2, 7, user, 'Withdraw', [{ type: 'UsageCovered', client: addr, amount: '250000', kind: 'outbound' }]),
        tx(3, 26, user, 'ReleaseTrade', [{ type: 'UsageCharged', client: addr, asset: 'USDT', amount: '100000', kind: 'release' }]),
        tx(4, (this.nowSecs - plan.started_at) / 3600, addr, 'BuyPlan', [
          { type: 'ClientPlanStarted', client: addr, tier: plan.tier, asset: 'USDT', fee: plan.fee_usd_micro, allowance: plan.allowance_usd_micro, ends_at: plan.ends_at },
        ]),
      ],
      next_cursor: null,
    });
  }
}
