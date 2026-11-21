/**
 * Shapes Harbor reads. The client, account, custody and params records come
 * from the node RPC (`chain/crates/keel-rpc`); the billing history comes
 * from the indexer's account transactions. Amounts are decimal strings in
 * smallest units: orcas for KEEL (1 KEEL = 1,000,000 orcas), USD micro for
 * prices, which is also the smallest unit of the billing asset.
 */
export type AmountStr = string;

export interface Tier {
  tier: number;
  lock_keel: AmountStr;
  fee_usd_micro: AmountStr;
  allowance_usd_micro: AmountStr;
  user_actions_per_day: number;
}

export interface Plan {
  tier: number;
  /** Block time, seconds. */
  started_at: number;
  ends_at: number;
  /** The period is running and the lock it was bought with is in place. */
  active: boolean;
  /** The period is running but the lock fell below `lock_keel`. */
  suspended: boolean;
  fee_usd_micro: AmountStr;
  allowance_usd_micro: AmountStr;
  allowance_left_usd_micro: AmountStr;
  lock_keel: AmountStr;
  /** Tier the plan renews at; 0 = it does not renew. */
  renew_tier: number;
  user_actions_per_day: number;
  user_actions_used_today: number;
}

export interface RetailFee {
  p2p_bps: number;
  taker_bps: number;
  withdraw_bps: number;
}

export interface AssetAmount {
  asset: string;
  amount: AmountStr;
}

export interface UsagePrices {
  address_usd_micro: AmountStr;
  outbound_usd_micro: AmountStr;
  /** Per float swap of a custodial client; absent on a node older than the knob. */
  swap_usd_micro?: AmountStr;
  fill_usd_micro: AmountStr;
  release_usd_micro: AmountStr;
  ruling_usd_micro: AmountStr;
}

/** `GET /v1/clients/{addr}` on the node RPC. Answered for any address. */
export interface ClientRecord {
  address: string;
  attester: boolean;
  fee: RetailFee | null;
  caps: RetailFee;
  earned: AssetAmount[];
  usage_paid: AssetAmount[];
  usage_owed: AmountStr;
  attested_accounts: number;
  protocol_fee_waived: boolean;
  usage_prices: UsagePrices;
  billing_asset: string;
  locked_keel: AmountStr;
  /** Highest tier the lock meets; 0 = none. */
  tier: number;
  plan: Plan | null;
  user_actions_per_user_per_day: number;
  plan_period_secs: number;
  tiers: Tier[];
}

export interface BalanceRow {
  asset: string;
  account_type: string;
  balance: AmountStr;
}

/** The part of `GET /v1/accounts/{addr}` (node RPC) Harbor uses. */
export interface AccountRecord {
  address: string;
  nonce: number;
  balances: BalanceRow[];
  budget: {
    locked: AmountStr;
    /** Unlocks on their way back to the balance; `ready_at` in ms. */
    unlocking: { ready_at: number; amount: AmountStr }[];
  };
}

export interface CustodyEntry {
  chain: string;
  gen: number;
  status: string;
  drain_until: number;
  reported_empty: number;
  vault: { epoch: number; public_key: string };
  signer_url: string;
  address: string | null;
  next_deposit_index: number;
  reserves: { asset: string; reserve: AmountStr; liabilities: AmountStr; halted: boolean }[];
}

/** `GET /v1/custody/{addr}` on the node RPC. */
export interface Custody {
  custodian: string;
  vaults: CustodyEntry[];
  rotations: { chain: string; gen: number; activate_at: number; signer_url: string }[];
  accounts: number;
  floats: { asset: string; custody: AmountStr; deposit: AmountStr }[];
}

export type EventRecord = { type: string } & Record<string, unknown>;

/** One row of the indexer's `GET /v1/accounts/{addr}/txs`. */
export interface HistoryTx {
  tx_id: string;
  height: number;
  /** ms */
  timestamp: number;
  signer: string;
  kind: string;
  ok: boolean;
  events: EventRecord[];
}

export interface HistoryPage {
  txs: HistoryTx[];
  next_cursor?: string | null;
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

/** Everything Harbor reads, so pages and tests share one seam. */
/** One price level of an order book: `price` is quote units per whole base coin, `size` is base units. */
export interface BookLevel {
  price: string;
  size: string;
  orders?: number;
}
/** `GET /v1/markets/{pair}/book`. `house` is the house maker's standing quote, as `[price, size]` per side. */
export interface Book {
  symbol: string;
  asks: BookLevel[];
  bids: BookLevel[];
  house: { ask?: [string | number, string | number] | null; bid?: [string | number, string | number] | null } | null;
}

export interface HarborApi {
  /** The resting orders of `pair`, best first. */
  book(pair: string): Promise<Book>;
  client(addr: string): Promise<ClientRecord>;
  account(addr: string): Promise<AccountRecord>;
  custody(addr: string): Promise<Custody>;
  /** `budget.unlock_delay_secs`, or null when the node does not report it. */
  unlockDelaySecs(): Promise<number | null>;
  history(addr: string, cursor?: string): Promise<HistoryPage>;
  /** Decimals per asset, from the indexer's asset list. */
  assets(): Promise<{ asset: string; decimals: number }[]>;
}
