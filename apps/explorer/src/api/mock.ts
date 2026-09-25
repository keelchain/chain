/**
 * In-browser mock of the indexer API, enabled with `VITE_API_MOCK=1`.
 *
 * Everything is derived from a seed (the network id), so URLs stay valid
 * across reloads: a block at any height is materialised lazily from
 * `hash(seed, height)`, and entity generators (offers, trades, proposals...)
 * "pin" the transactions that created them to their heights so links between
 * pages resolve. The tip advances on a timer to feed the live views.
 *
 * Amount and event shapes follow docs/explorer-api.md and the serde
 * output of `keel_actions::Action` / `keel_vm::Event`.
 */
import {
  ApiError,
  type Account,
  type Asset,
  type AssetDetail,
  type Block,
  type BlockDetail,
  type BlocksPage,
  type Candle,
  type CandleInterval,
  type Deposit,
  type DepositsPage,
  type Epoch,
  type EventRecord,
  type ExplorerApi,
  type Fill,
  type FillsPage,
  type Health,
  type LightningStatus,
  type Market,
  type MarketDetail,
  type Offer,
  type OfferDetail,
  type OffersPage,
  type OffersQuery,
  type Order,
  type Outbound,
  type OutboundBatch,
  type OutboundsPage,
  type Params,
  type ParamChange,
  type Proposal,
  type ProposalDetail,
  type SearchResult,
  type Stats,
  type Trade,
  type TradeHistoryEntry,
  type Transfer,
  type TransfersPage,
  type Tx,
  type TxDetail,
  type TxsPage,
  type TxsQuery,
  type Validator,
  type Vault,
  type Vote,
  type WsMessage,
} from './types';

// ---------------------------------------------------------------- PRNG

function mix(a: number, b: number): number {
  let h = (a ^ 0x9e3779b9) >>> 0;
  h = Math.imul(h ^ (b >>> 0), 0x85ebca6b) >>> 0;
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35) >>> 0;
  h ^= h >>> 16;
  return h >>> 0;
}

function seedOf(...parts: (number | string)[]): number {
  let h = 0x811c9dc5;
  for (const p of parts) {
    if (typeof p === 'number') {
      h = mix(h, Math.floor(p));
      h = mix(h, Math.floor(p / 0x100000000));
    } else {
      for (let i = 0; i < p.length; i++) h = mix(h, p.charCodeAt(i));
    }
  }
  return h >>> 0;
}

class Rng {
  private s: number;
  constructor(seed: number) {
    this.s = seed >>> 0 || 1;
  }
  next(): number {
    let t = (this.s += 0x6d2b79f5);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }
  int(min: number, max: number): number {
    return min + Math.floor(this.next() * (max - min + 1));
  }
  pick<T>(arr: readonly T[]): T {
    const v = arr[Math.floor(this.next() * arr.length)];
    if (v === undefined) throw new Error('empty pick');
    return v;
  }
  chance(p: number): boolean {
    return this.next() < p;
  }
  hex(bytes: number): string {
    let s = '';
    for (let i = 0; i < bytes; i++) s += this.int(0, 255).toString(16).padStart(2, '0');
    return s;
  }
  base58(len: number): string {
    const A = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
    let s = '';
    for (let i = 0; i < len; i++) s += A[this.int(0, A.length - 1)];
    return s;
  }
  bech32(len: number): string {
    const A = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
    let s = '';
    for (let i = 0; i < len; i++) s += A[this.int(0, A.length - 1)];
    return s;
  }
}

// ---------------------------------------------------------------- amounts

function units(whole: number, decimals: number): string {
  // whole may be fractional; keep 9 significant fractional digits.
  const scaled = BigInt(Math.round(whole * 1e9));
  if (decimals >= 9) return (scaled * 10n ** BigInt(decimals - 9)).toString();
  return (scaled / 10n ** BigInt(9 - decimals)).toString();
}

function toWhole(raw: string, decimals: number): number {
  return Number(BigInt(raw)) / 10 ** decimals;
}

function bpsOf(raw: string, bps: number): string {
  return ((BigInt(raw) * BigInt(bps)) / 10_000n).toString();
}

function quoteOf(quantity: string, price: string, baseDecimals: number): string {
  return ((BigInt(quantity) * BigInt(price)) / 10n ** BigInt(baseDecimals)).toString();
}

function sum(values: string[]): string {
  return values.reduce((a, v) => a + BigInt(v), 0n).toString();
}

// ---------------------------------------------------------------- static config

interface AssetDef {
  asset: string;
  decimals: number;
  kind: 'native' | 'stable' | 'vault';
  chain?: string;
  usd: number;
}

const ASSETS: AssetDef[] = [
  { asset: 'KEEL', decimals: 6, kind: 'native', usd: 0.42 },
  { asset: 'KUSD', decimals: 6, kind: 'stable', usd: 1 },
  { asset: 'BTC.BTC', decimals: 8, kind: 'vault', chain: 'BTC', usd: 62_400 },
  { asset: 'ETH.ETH', decimals: 18, kind: 'vault', chain: 'ETH', usd: 3_120 },
  { asset: 'ETH.USDT', decimals: 6, kind: 'vault', chain: 'ETH', usd: 1 },
  { asset: 'ETH.USDC', decimals: 6, kind: 'vault', chain: 'ETH', usd: 1 },
  { asset: 'TRON.USDT', decimals: 6, kind: 'vault', chain: 'TRON', usd: 1 },
  { asset: 'TRON.TRX', decimals: 6, kind: 'vault', chain: 'TRON', usd: 0.118 },
];

const ASSET_BY_NAME = new Map(ASSETS.map((a) => [a.asset, a]));

interface MarketDef {
  pair: string;
  base: string;
  quote: string;
  price: number;
  tick: number;
  vol: number;
}

const MARKETS: MarketDef[] = [
  { pair: 'BTC-KUSD', base: 'BTC.BTC', quote: 'KUSD', price: 62_400, tick: 1, vol: 0.012 },
  { pair: 'ETH-KUSD', base: 'ETH.ETH', quote: 'KUSD', price: 3_120, tick: 0.1, vol: 0.016 },
  { pair: 'KEEL-KUSD', base: 'KEEL', quote: 'KUSD', price: 0.42, tick: 0.0001, vol: 0.03 },
  { pair: 'TRX-KUSD', base: 'TRON.TRX', quote: 'KUSD', price: 0.118, tick: 0.00001, vol: 0.02 },
];

const CHAINS = ['BTC', 'ETH', 'TRON'] as const;
const CHAIN_ENUM: Record<string, string> = { BTC: 'Bitcoin', ETH: 'Ethereum', TRON: 'Tron' };
const CHAIN_NATIVE: Record<string, string> = { BTC: 'BTC.BTC', ETH: 'ETH.ETH', TRON: 'TRON.TRX' };

const FIATS = ['USD', 'EUR', 'GBP', 'NGN', 'EGP', 'INR', 'BRL', 'TRY'];
const PAYMENT_METHODS = ['Bank transfer', 'Revolut', 'Wise', 'PayPal', 'Cash deposit', 'M-Pesa', 'Vodafone Cash', 'SEPA instant', 'Zelle', 'Amazon gift card'];
const COUNTRIES = ['US', 'DE', 'GB', 'NG', 'EG', 'IN', 'BR', 'TR', 'FR', 'ES'];

const ERRORS: { code: string; message: string }[] = [
  { code: 'NOT_ENOUGH_FUNDS', message: 'insufficient available balance' },
  { code: 'BUDGET_EXHAUSTED', message: 'address action budget exhausted; fill volume or buy budget' },
  { code: 'INVALID', message: 'price not a multiple of tick size' },
  { code: 'NOT_FOUND', message: 'order not found or already closed' },
  { code: 'BAD_NONCE', message: 'expected nonce 1842, got 1841' },
  { code: 'BLOCK_CAP', message: 'per-block action cap reached for address' },
];

const DEFAULT_PARAMS: Record<string, string> = {
  taker_fee_bps: '10',
  maker_fee_bps: '0',
  referral_share_bps: '2000',
  withdraw_flat_fee_usd_micro: '1000000',
  dispute_fee_bps: '100',
  fee_split_treasury_bps: '5000',
  fee_split_validators_bps: '4000',
  fee_split_burn_bps: '1000',
  offer_deposit: '10000000',
  default_payment_window_secs: '1800',
  release_grace_secs: '3600',
  ruling_window_secs: '86400',
  offer_allowance_base: '2',
  offer_allowance_trades_per_step: '10',
  offer_allowance_step: '2',
  min_validator_bond: '100000000000',
  min_observer_bond: '500000000000',
  min_arbitrator_bond: '50000000000',
  unbonding_blocks: '100000',
  max_validators: '100',
  epoch_length_blocks: '10000',
  observer_reward_bps: '2500',
  slash_double_sign_bps: '500',
  slash_false_observation_bps: '10000',
  proposal_deposit: '1000000000000',
  voting_period_blocks: '20000',
  timelock_blocks: '5000',
  quorum_bps: '3340',
  threshold_bps: '5000',
  veto_bps: '3340',
  observer_quorum_bps: '6667',
  confirmations_btc: '2',
  confirmations_eth: '12',
  confirmations_tron: '19',
  deposit_daily_cap_usd_micro: '0',
  large_deposit_usd_micro: '50000000000',
  large_deposit_delay_blocks: '600',
  outbound_batch_interval_blocks: '20',
  house_quote_max_ttl_blocks: '60',
  max_open_orders_per_pair: '500',
  'budget.base': '10000',
  'budget.per_usd_filled': '1',
  'budget.cancel_bonus': '100000',
  'budget.max_per_block': '50',
  'budget.price_per_action': '1000',
};

// ---------------------------------------------------------------- action specs

/** A transaction before it is stamped with height/index/id. */
interface ActionSpec {
  signer: string;
  kind: string;
  module: string;
  action: Record<string, unknown> | string;
  events: EventRecord[];
  ok: boolean;
  error?: { code: string; message: string };
  fills?: { pair: string; fill: Omit<Fill, 'tx_id' | 'height' | 'timestamp'> }[];
}

export interface MockOptions {
  /** Simulated request latency in ms (0 in tests). */
  latencyMs?: number;
  /** Wall-clock ms between mock blocks. */
  blockMs?: number;
  /** Fixed "now" (ms) for deterministic tests; defaults to Date.now(). */
  now?: number;
  /** Whether the tip advances with wall-clock time. */
  live?: boolean;
}

export class MockExplorerApi implements ExplorerApi {
  readonly baseUrl: string;
  readonly networkId: string;
  private readonly seed: number;
  private readonly latency: number;
  private readonly blockMs: number;
  private readonly live: boolean;
  private readonly t0: number;
  private readonly h0: number;
  private readonly chainId: string;

  private readonly accounts: string[];
  private readonly validatorsList: Validator[];
  private readonly observers: string[];
  private readonly arbitrators: string[];
  private readonly attesters: string[];
  private readonly paramAdmin: string;
  private readonly house: string;

  private readonly offersList: Offer[] = [];
  private readonly tradesList: Trade[] = [];
  private readonly proposalsList: ProposalDetail[] = [];
  private readonly paramHistory: ParamChange[] = [];
  private readonly paramsNow: Record<string, string> = { ...DEFAULT_PARAMS };
  private readonly depositsList: Deposit[] = [];
  private readonly outboundsList: Outbound[] = [];
  private readonly batchesList: OutboundBatch[] = [];
  private readonly ordersList: Order[] = [];
  private readonly vaultsList: Vault[] = [];

  private readonly pinned = new Map<number, ActionSpec[]>();
  private readonly blockCache = new Map<number, BlockDetail>();
  private readonly txIndex = new Map<string, { height: number; index: number }>();
  private readonly actionsCache = new Map<number, ActionSpec[]>();
  private readonly candleCache = new Map<string, Candle[]>();
  private readonly listeners = new Set<(m: WsMessage) => void>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private lastEmitted: number;

  constructor(networkId = 'testnet', opts: MockOptions = {}) {
    this.networkId = networkId;
    this.baseUrl = `mock://${networkId}`;
    this.seed = seedOf('keel-mock', networkId);
    this.latency = opts.latencyMs ?? 120;
    this.blockMs = opts.blockMs ?? 600;
    this.live = opts.live ?? true;
    this.t0 = opts.now ?? Date.now();
    this.h0 = networkId === 'mainnet' ? 2_418_930 : 1_842_317;
    this.chainId = networkId === 'mainnet' ? 'keel-1' : 'keel-testnet-3';
    this.lastEmitted = this.h0;

    const rng = new Rng(seedOf(this.seed, 'accounts'));
    this.accounts = Array.from({ length: 96 }, () => rng.hex(32));
    this.validatorsList = this.accounts.slice(0, 9).map((address, i) => this.makeValidator(address, i));
    this.observers = this.accounts.slice(9, 16);
    this.arbitrators = this.accounts.slice(16, 19);
    this.attesters = this.accounts.slice(19, 21);
    this.paramAdmin = this.accounts[21] ?? this.accounts[0]!;
    this.house = this.accounts[22] ?? this.accounts[0]!;

    this.genOffersAndTrades();
    this.genOrders();
    this.genGovernance();
    this.genVaults();
  }

  // ------------------------------------------------------------ chain clock

  private tip(): number {
    if (!this.live) return this.h0;
    return this.h0 + Math.floor((Date.now() - this.t0) / this.blockMs);
  }

  private timestampOf(height: number): number {
    return this.t0 - (this.h0 - height) * this.blockMs;
  }

  private heightAt(msAgo: number): number {
    return Math.max(1, this.h0 - Math.round(msAgo / this.blockMs));
  }

  private delay<T>(v: () => T): Promise<T> {
    return new Promise((resolve, reject) => {
      const run = () => {
        try {
          resolve(v());
        } catch (e) {
          reject(e);
        }
      };
      if (this.latency <= 0) run();
      else setTimeout(run, this.latency + Math.random() * this.latency * 0.5);
    });
  }

  private user(rng: Rng): string {
    return this.accounts[rng.int(23, this.accounts.length - 1)]!;
  }

  private pin(height: number, spec: ActionSpec): void {
    const list = this.pinned.get(height) ?? [];
    list.push(spec);
    this.pinned.set(height, list);
  }

  private txIdAt(height: number, index: number): string {
    return new Rng(seedOf(this.seed, 'txid', height, index)).hex(32);
  }

  // ------------------------------------------------------------ generators

  private makeValidator(address: string, i: number): Validator {
    const rng = new Rng(seedOf(this.seed, 'validator', i));
    const selfBond = units(rng.int(150_000, 900_000), 6);
    const delegated = units(rng.int(50_000, 2_500_000), 6);
    return {
      address,
      consensus_key: rng.hex(32),
      self_bond: selfBond,
      delegated,
      power: sum([selfBond, delegated]),
      jailed: i === 7,
      blocks_proposed_24h: i === 7 ? 0 : rng.int(12_000, 18_000),
      uptime: i === 7 ? 0.62 : 0.985 + rng.next() * 0.015,
    };
  }

  private offerSpec(rng: Rng, o: Offer): Record<string, unknown> {
    return {
      side: o.side,
      asset: o.asset,
      fiat_currency: o.fiat_currency,
      payment_method: o.payment_method,
      margin_bps: o.margin_bps,
      fixed_price: null,
      min_amount: o.min_amount,
      max_amount: o.max_amount,
      payment_window_secs: o.payment_window_secs,
      country: o.country,
      min_tier: o.min_tier,
      terms: rng.pick(['Verified buyers only. Send from an account in your own name.', 'Fast release during working hours (09:00-22:00 UTC).', 'No third-party payments. Reference: trade id.', 'Cash deposit at branch, receipt photo required.']),
      instructions_hash: rng.hex(32),
    };
  }

  private genOffersAndTrades(): void {
    const rng = new Rng(seedOf(this.seed, 'offers'));
    const dayMs = 86_400_000;
    for (let id = 1; id <= 48; id++) {
      const asset = rng.pick(['BTC.BTC', 'ETH.ETH', 'ETH.USDT', 'TRON.USDT', 'KEEL']);
      const def = ASSET_BY_NAME.get(asset)!;
      const owner = this.user(rng);
      const minUsd = rng.int(20, 200);
      const maxUsd = minUsd * rng.int(5, 40);
      const offer: Offer = {
        id,
        owner,
        side: rng.chance(0.5) ? 'buy' : 'sell',
        asset,
        fiat_currency: rng.pick(FIATS),
        payment_method: rng.pick(PAYMENT_METHODS),
        margin_bps: rng.int(-150, 600),
        min_amount: units(minUsd / def.usd, def.decimals),
        max_amount: units(maxUsd / def.usd, def.decimals),
        payment_window_secs: rng.pick([900, 1800, 3600]),
        country: rng.chance(0.7) ? rng.pick(COUNTRIES) : null,
        min_tier: rng.pick([0, 0, 1, 2]),
        status: rng.chance(0.7) ? 'active' : rng.chance(0.5) ? 'paused' : 'closed',
        created_height: this.heightAt(rng.int(1, 30) * dayMs + rng.int(0, dayMs)),
      };
      this.offersList.push(offer);
      this.pin(offer.created_height, {
        signer: owner,
        kind: 'CreateOffer',
        module: 'market_p2p',
        action: { CreateOffer: this.offerSpec(rng, offer) },
        events: [{ type: 'OfferCreated', offer_id: id, owner }],
        ok: true,
      });
    }

    let tradeId = 1;
    for (const offer of this.offersList) {
      const n = rng.int(0, 6);
      for (let k = 0; k < n; k++) {
        const def = ASSET_BY_NAME.get(offer.asset)!;
        const taker = this.user(rng);
        const buyer = offer.side === 'sell' ? taker : offer.owner;
        const seller = offer.side === 'sell' ? offer.owner : taker;
        const amountWhole = toWhole(offer.min_amount, def.decimals) * (1 + rng.next() * 8);
        const amount = units(amountWhole, def.decimals);
        const fiatRate = offer.fiat_currency === 'NGN' ? 1550 : offer.fiat_currency === 'EGP' ? 48 : offer.fiat_currency === 'INR' ? 84 : 1;
        const fiat = units(amountWhole * def.usd * fiatRate * (1 + offer.margin_bps / 10_000), 2);
        const startedAgo = rng.int(0, Math.max(1, Math.round((this.h0 - offer.created_height) * this.blockMs)));
        const started = Math.min(this.h0 - 5, Math.max(offer.created_height + 1, this.heightAt(startedAgo)));
        const windowBlocks = Math.round((offer.payment_window_secs * 1000) / this.blockMs);
        const usdVal = amountWhole * def.usd;
        const feeBps = usdVal < 50 ? 200 : 100;
        const fee = bpsOf(amount, feeBps);
        const roll = rng.next();
        const status = roll < 0.55 ? 'released' : roll < 0.68 ? 'cancelled' : roll < 0.78 ? 'paid' : roll < 0.86 ? 'funded' : roll < 0.93 ? 'disputed' : 'ruled';
        const history: TradeHistoryEntry[] = [];
        const push = (s: string, h: number, by?: string) => {
          history.push({ status: s, height: h, timestamp: this.timestampOf(h), tx_id: this.txIdAt(h, 0), by });
        };
        push('funded', started, taker);
        const paidH = started + rng.int(20, Math.max(21, windowBlocks - 10));
        let closed: number | null = null;
        let paidAt: number | null = null;
        let dispute: Trade['dispute'] = null;
        if (status === 'cancelled') {
          closed = started + rng.int(5, windowBlocks + 50);
          push('cancelled', closed, rng.chance(0.6) ? buyer : seller);
        } else if (status !== 'funded') {
          paidAt = this.timestampOf(paidH);
          push('paid', paidH, buyer);
          if (status === 'released') {
            closed = paidH + rng.int(10, 4000);
            push('released', closed, seller);
          } else if (status === 'disputed' || status === 'ruled') {
            const dh = paidH + rng.int(200, 8000);
            const by = rng.chance(0.7) ? buyer : seller;
            push('disputed', dh, by);
            const evidence = [{ by, hash: rng.hex(32), height: dh }];
            if (rng.chance(0.6)) evidence.push({ by: by === buyer ? seller : buyer, hash: rng.hex(32), height: dh + rng.int(10, 500) });
            let ruling: NonNullable<Trade['dispute']>['ruling'] = null;
            if (status === 'ruled') {
              closed = dh + rng.int(600, 12_000);
              const kindRoll = rng.next();
              const kind = kindRoll < 0.5 ? 'WinsBuyer' : kindRoll < 0.85 ? 'WinsSeller' : 'Split';
              const buyerBps = kind === 'WinsBuyer' ? 10_000 : kind === 'WinsSeller' ? 0 : rng.pick([2500, 5000, 7500]);
              ruling = {
                kind,
                buyer_bps: buyerBps,
                buyer_amount: bpsOf(amount, buyerBps),
                seller_amount: bpsOf(amount, 10_000 - buyerBps),
                height: closed,
                arbitrator: rng.pick(this.arbitrators),
              };
              push('ruled', closed, ruling.arbitrator);
            }
            dispute = { opened_by: by, opened_height: dh, evidence, ruling };
          }
        }
        // Anything that would land in the future collapses to "still open".
        const trade: Trade = {
          id: tradeId++,
          offer_id: offer.id,
          buyer,
          seller,
          asset: offer.asset,
          amount,
          fee,
          fiat_amount: fiat,
          fiat_currency: offer.fiat_currency,
          status,
          started_height: started,
          deadline: this.timestampOf(started) + offer.payment_window_secs * 1000,
          paid_at: paidAt,
          closed_height: closed,
          dispute,
          history,
        };
        this.tradesList.push(trade);
        // Pin the linked transactions so history rows resolve to real txs.
        for (const h of history) {
          const spec = this.tradeSpec(trade, h, offer);
          if (spec) this.pin(h.height, spec);
        }
      }
    }
    this.tradesList.sort((a, b) => b.started_height - a.started_height);
  }

  private tradeSpec(t: Trade, h: TradeHistoryEntry, offer: Offer): ActionSpec | null {
    const by = h.by ?? t.buyer;
    switch (h.status) {
      case 'funded':
        return {
          signer: by,
          kind: 'StartTrade',
          module: 'market_p2p',
          action: { StartTrade: { offer_id: t.offer_id, amount: t.amount, fiat_amount: t.fiat_amount, instructions_hash: new Rng(seedOf(this.seed, 'ih', t.id)).hex(32) } },
          events: [
            { type: 'Transferred', from: t.seller, to: `escrow:trade:${t.id}`, asset: t.asset, amount: t.amount },
            { type: 'TradeStarted', trade_id: t.id, offer_id: t.offer_id, buyer: t.buyer, seller: t.seller, amount: t.amount },
          ],
          ok: true,
        };
      case 'paid':
        return {
          signer: t.buyer,
          kind: 'MarkPaid',
          module: 'market_p2p',
          action: { MarkPaid: { trade_id: t.id, proof_hash: new Rng(seedOf(this.seed, 'ph', t.id)).hex(32) } },
          events: [{ type: 'TradePaid', trade_id: t.id }],
          ok: true,
        };
      case 'released': {
        const toBuyer = (BigInt(t.amount) - BigInt(t.fee)).toString();
        return {
          signer: t.seller,
          kind: 'ReleaseTrade',
          module: 'market_p2p',
          action: { ReleaseTrade: { trade_id: t.id } },
          events: [
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: t.buyer, asset: t.asset, amount: toBuyer },
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: 'system:treasury', asset: t.asset, amount: bpsOf(t.fee, 5000) },
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: 'system:validator_rewards', asset: t.asset, amount: bpsOf(t.fee, 4000) },
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: 'system:burn', asset: t.asset, amount: bpsOf(t.fee, 1000) },
            { type: 'TradeReleased', trade_id: t.id, to_buyer: toBuyer, fee: t.fee },
          ],
          ok: true,
        };
      }
      case 'cancelled':
        return {
          signer: by,
          kind: 'CancelTrade',
          module: 'market_p2p',
          action: { CancelTrade: { trade_id: t.id } },
          events: [
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: t.seller, asset: t.asset, amount: t.amount },
            { type: 'TradeCancelled', trade_id: t.id, by },
          ],
          ok: true,
        };
      case 'disputed':
        return {
          signer: by,
          kind: 'OpenDispute',
          module: 'disputes',
          action: { OpenDispute: { trade_id: t.id, evidence_hash: t.dispute?.evidence[0]?.hash ?? '' } },
          events: [{ type: 'DisputeOpened', trade_id: t.id, by }],
          ok: true,
        };
      case 'ruled': {
        const r = t.dispute?.ruling;
        if (!r) return null;
        const ruling = r.kind === 'Split' ? { Split: { buyer_bps: r.buyer_bps } } : r.kind;
        return {
          signer: r.arbitrator ?? by,
          kind: 'RuleDispute',
          module: 'disputes',
          action: { RuleDispute: { trade_id: t.id, ruling } },
          events: [
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: t.buyer, asset: t.asset, amount: r.buyer_amount },
            { type: 'Transferred', from: `escrow:trade:${t.id}`, to: t.seller, asset: t.asset, amount: r.seller_amount },
            { type: 'DisputeRuled', trade_id: t.id, buyer_amount: r.buyer_amount, seller_amount: r.seller_amount },
          ],
          ok: true,
        };
      }
      default:
        return null;
    }
    void offer;
  }

  private genOrders(): void {
    const rng = new Rng(seedOf(this.seed, 'orders'));
    for (let id = 1; id <= 400; id++) {
      const m = rng.pick(MARKETS);
      const base = ASSET_BY_NAME.get(m.base)!;
      const quote = ASSET_BY_NAME.get(m.quote)!;
      const side = rng.chance(0.5) ? 'buy' : 'sell';
      const priceWhole = m.price * (1 + (rng.next() - 0.5) * 0.02);
      const price = units(priceWhole, quote.decimals);
      const qtyUsd = rng.int(10, 5000);
      const quantity = units(qtyUsd / base.usd, base.decimals);
      const fillRatio = rng.pick([0, 0, 0.3, 0.5, 1, 1, 1]);
      const filled = bpsOf(quantity, Math.round(fillRatio * 10_000));
      const created = this.heightAt(rng.int(0, 3 * 86_400_000));
      const status = fillRatio === 1 ? 'filled' : rng.chance(0.4) ? 'cancelled' : fillRatio > 0 ? 'partially_filled' : 'open';
      const fills: Fill[] = [];
      if (fillRatio > 0) {
        const n = rng.int(1, 3);
        let left = BigInt(filled);
        for (let i = 0; i < n; i++) {
          const q = i === n - 1 ? left : left / BigInt(n - i);
          left -= q;
          const h = created + rng.int(0, 40);
          const fillQuote = quoteOf(q.toString(), price, base.decimals);
          fills.push({
            tx_id: this.txIdAt(h, 0),
            height: h,
            timestamp: this.timestampOf(h),
            price,
            quantity: q.toString(),
            quote: fillQuote,
            taker: side === 'buy' ? this.accounts[id % 70 + 23]! : this.accounts[(id * 7) % 70 + 23]!,
            taker_side: side,
            maker_order_id: rng.chance(0.85) ? rng.int(1, 400) : null,
            fee: side === 'buy' ? bpsOf(q.toString(), 10) : bpsOf(fillQuote, 10),
          });
        }
      }
      this.ordersList.push({
        id,
        owner: this.accounts[(id * 13) % 70 + 23]!,
        pair: m.pair,
        side,
        order_type: rng.chance(0.85) ? 'limit' : 'market',
        price,
        quantity,
        filled,
        status,
        created_height: created,
        tx_id: this.txIdAt(created, 0),
        fills,
      });
    }
  }

  private genGovernance(): void {
    const rng = new Rng(seedOf(this.seed, 'gov'));
    const dayMs = 86_400_000;
    const kinds: { title: string; description: string; kind: Record<string, unknown> | string; status: string; key?: string; value?: string }[] = [
      { title: 'Lower taker fee to 8 bps on BTC-KUSD', description: 'Competitive pressure from centralized venues; maker stays at 0. Revenue impact modelled at -12% with expected +25% volume.', kind: { ParamChange: { key: 'taker_fee_bps', value: 8 } }, status: 'voting' },
      { title: 'List USDC-KUSD', description: 'Adds a second stable-to-stable pair for reserve rebalancing.', kind: { ListPair: { symbol: 'USDC-KUSD', base_asset: 'ETH.USDC', quote_asset: 'KUSD', base_decimals: 6, quote_decimals: 6, tick_size: '100', lot_size: '1000000', min_notional: '1000000', max_notional: '100000000000000', taker_fee_bps: 2, maker_fee_bps: 0, enabled: true, house_maker_enabled: false } }, status: 'passed' },
      { title: 'Raise Bitcoin confirmation depth to 3', description: 'Post-incident hardening following the 2-block reorg observed on the observer set on 2026-08-30.', kind: { ParamChange: { key: 'confirmations_btc', value: 3 } }, status: 'executed', key: 'confirmations_btc', value: '3' },
      { title: 'Treasury grant: explorer and SDK maintenance (Q4)', description: 'Fund 6 months of maintenance for the explorer, TS SDK and indexer from treasury.', kind: { TreasurySpend: { to: this.accounts[40], asset: 'KUSD', amount: units(48_000, 6) } }, status: 'executed' },
      { title: 'Software upgrade v0.9.0 at height 1,900,000', description: 'Alpenglow-style fast path for the consensus adapter; unupgraded nodes halt at the named height.', kind: { SoftwareUpgrade: { version: 'v0.9.0', height: 1_900_000 } }, status: 'voting' },
      { title: 'Add TRON.USDT to the stablecoin basket with a 2M cap', description: 'Diversifies reserve issuers; cap keeps Tron below 25% of the basket.', kind: { SetStableBasket: { asset: 'TRON.USDT', cap: units(2_000_000, 6), enabled: true } }, status: 'executed' },
      { title: 'Set observer set to 7 with threshold 5', description: 'Rotate in two new observer-signers next epoch.', kind: { SetObservers: { members: this.observers, threshold: 5 } }, status: 'executed' },
      { title: 'Increase proposal deposit to 2,000,000 KEEL', description: 'Spam proposals during the last voting period.', kind: { ParamChange: { key: 'proposal_deposit', value: '2000000000000' } }, status: 'rejected' },
      { title: 'Pause TRON vault for 24h', description: 'Emergency pause after observer disagreement on a TRON block; superseded by validator emergency pause.', kind: { PauseModule: { module: 'vaults.TRON', until_height: 1_700_000 } }, status: 'vetoed' },
      { title: 'Signal: adopt KUSD as the unit of account for offers', description: 'Text proposal signalling the migration off USDT-denominated offers.', kind: 'Text', status: 'executed' },
      { title: 'Raise treasury fee split to 55%', description: 'Failed at execution: would leave BTC.BTC reserves below liabilities after the pending treasury spend.', kind: { ParamChange: { key: 'fee_split_treasury_bps', value: 5500 } }, status: 'failed' },
      { title: 'Revoke the parameter admin', description: 'Hand all parameter changes to governance; the platform super admin key is revoked.', kind: { SetParamAdmin: { admin: null } }, status: 'voting' },
    ];
    const bonded = units(31_500_000, 6);
    kinds.forEach((k, i) => {
      const id = i + 1;
      const submitted = this.heightAt((kinds.length - i) * 4 * dayMs + rng.int(0, dayMs));
      const votingEnd = submitted + 20_000;
      const votes: Vote[] = [];
      const nVotes = rng.int(6, 22);
      for (let v = 0; v < nVotes; v++) {
        const choice = k.status === 'vetoed' ? rng.pick(['veto', 'veto', 'no', 'yes']) : k.status === 'rejected' ? rng.pick(['no', 'no', 'yes', 'abstain']) : rng.pick(['yes', 'yes', 'yes', 'no', 'abstain']);
        const h = submitted + rng.int(1, Math.min(20_000, Math.max(2, this.h0 - submitted - 1)));
        const voter = v < 9 ? this.validatorsList[v]!.address : this.user(rng);
        const weight = v < 9 ? this.validatorsList[v]!.power : units(rng.int(1000, 400_000), 6);
        votes.push({ voter, choice: choice as Vote['choice'], weight, height: h, tx_id: this.txIdAt(h, 0) });
        this.pin(h, {
          signer: voter,
          kind: 'Vote',
          module: 'gov',
          action: { Vote: { proposal_id: id, choice: choice.charAt(0).toUpperCase() + choice.slice(1) } },
          events: [{ type: 'Voted', proposal_id: id, voter, weight }],
          ok: true,
        });
      }
      const tally = { yes: '0', no: '0', abstain: '0', veto: '0', total_bonded: bonded };
      for (const v of votes) tally[v.choice] = sum([tally[v.choice], v.weight]);
      const proposal: ProposalDetail = {
        id,
        proposer: k.status === 'voting' && i === 0 ? this.paramAdmin : this.user(rng),
        title: k.title,
        description: k.description,
        kind: k.kind,
        status: k.status,
        deposit: DEFAULT_PARAMS['proposal_deposit']!,
        submitted_height: submitted,
        voting_end_height: votingEnd,
        execute_height: k.status === 'executed' || k.status === 'failed' ? votingEnd + 5_000 : null,
        tally,
        votes: votes.sort((a, b) => b.height - a.height),
      };
      if (k.status === 'voting') {
        proposal.voting_end_height = this.h0 + rng.int(2_000, 18_000);
      }
      this.proposalsList.push(proposal);
      this.pin(submitted, {
        signer: proposal.proposer,
        kind: 'Propose',
        module: 'gov',
        action: { Propose: { title: k.title, description: k.description, kind: k.kind } },
        events: [
          { type: 'Transferred', from: proposal.proposer, to: `gov:deposit:${id}`, asset: 'KEEL', amount: proposal.deposit },
          { type: 'ProposalCreated', proposal_id: id, proposer: proposal.proposer },
        ],
        ok: true,
      });
      if (proposal.execute_height) {
        const okExec = k.status === 'executed';
        const events: EventRecord[] = [{ type: 'ProposalExecuted', proposal_id: id, ok: okExec }];
        if (okExec && k.key && k.value) {
          events.unshift({ type: 'ParamChanged', key: k.key, value: k.value });
          this.paramHistory.push({ height: proposal.execute_height, key: k.key, from: this.paramsNow[k.key] ?? '0', to: k.value, tx_id: this.txIdAt(proposal.execute_height, 0) });
          this.paramsNow[k.key] = k.value;
        }
        this.pin(proposal.execute_height, {
          signer: proposal.proposer,
          kind: 'ExecuteProposal',
          module: 'gov',
          action: { ExecuteProposal: { proposal_id: id } },
          events,
          ok: true,
        });
      }
    });
    // A few direct SetParam changes by the param admin (launch-phase tuning).
    const direct: [string, string][] = [
      ['taker_fee_bps', '12'],
      ['taker_fee_bps', '10'],
      ['large_deposit_delay_blocks', '600'],
      ['outbound_batch_interval_blocks', '20'],
    ];
    direct.forEach(([key, value], i) => {
      const h = this.heightAt((40 - i * 6) * dayMs + rng.int(0, dayMs));
      const from = i === 0 ? '20' : this.paramHistory.filter((p) => p.key === key).at(-1)?.to ?? DEFAULT_PARAMS[key] ?? '0';
      this.paramHistory.push({ height: h, key, from, to: value, tx_id: this.txIdAt(h, 0) });
      this.pin(h, {
        signer: this.paramAdmin,
        kind: 'SetParam',
        module: 'gov',
        action: { SetParam: { key, value } },
        events: [{ type: 'ParamChanged', key, value }],
        ok: true,
      });
    });
    this.paramHistory.sort((a, b) => b.height - a.height);
    this.proposalsList.sort((a, b) => b.id - a.id);
  }

  private genVaults(): void {
    const rng = new Rng(seedOf(this.seed, 'vaults'));
    const epoch = Math.floor(this.h0 / 10_000);
    const feeRates: Record<string, number> = { BTC: 14, ETH: 9, TRON: 420 };
    for (const chain of CHAINS) {
      const assets = ASSETS.filter((a) => a.chain === chain);
      const reserves = assets.map((a) => {
        const usd = rng.int(400_000, 3_200_000);
        return { asset: a.asset, amount: units(usd / a.usd, a.decimals) };
      });
      const liabilities = reserves.map((r) => ({ asset: r.asset, amount: bpsOf(r.amount, rng.int(8_600, 9_950)) }));
      this.vaultsList.push({
        chain,
        epoch,
        address_count: rng.int(1_200, 9_800),
        reserves,
        liabilities,
        fee_rate: feeRates[chain] ?? 1,
        halted: false,
        threshold: 5,
        signers: this.observers,
      });
    }
    // Deposits
    let depIdx = 0;
    for (const chain of CHAINS) {
      const assets = ASSETS.filter((a) => a.chain === chain);
      const required = chain === 'BTC' ? 3 : chain === 'ETH' ? 12 : 19;
      for (let i = 0; i < 28; i++) {
        const a = rng.pick(assets);
        const usd = rng.chance(0.08) ? rng.int(60_000, 250_000) : rng.int(15, 8_000);
        const amount = units(usd / a.usd, a.decimals);
        const ageBlocks = rng.int(3, 20_000);
        const first = this.h0 - ageBlocks;
        const roll = rng.next();
        const status: Deposit['status'] = ageBlocks < 60 ? 'pending' : roll < 0.9 ? 'credited' : roll < 0.96 ? 'held' : 'rejected';
        const depth = status === 'pending' ? rng.int(0, required - 1) : required + rng.int(0, 400);
        const owner = this.user(rng);
        const txHash = chain === 'ETH' ? `0x${rng.hex(32)}` : rng.hex(32);
        const d: Deposit = {
          chain,
          asset: a.asset,
          tx_hash: txHash,
          index: rng.int(0, 3),
          owner,
          deposit_index: rng.int(1, 9000),
          amount,
          external_height: chain === 'BTC' ? 912_400 + rng.int(0, 300) : chain === 'ETH' ? 23_450_000 + rng.int(0, 5000) : 75_800_000 + rng.int(0, 20_000),
          depth,
          required_depth: required,
          votes: status === 'pending' ? rng.int(1, 4) : rng.int(5, 7),
          status,
          first_height: first,
          last_height: first + rng.int(0, Math.min(ageBlocks, 40)),
          release_height: status === 'held' ? first + 600 : null,
        };
        this.depositsList.push(d);
        depIdx++;
        // Observer attestation transactions.
        const observer = this.observers[depIdx % this.observers.length]!;
        const proof = chain === 'BTC' ? { Bitcoin: { headers: `<${(depth + 1) * 80} bytes>`, merkle_proof: '<partial merkle tree>', tx_index: rng.int(0, 2500) } } : chain === 'ETH' ? { Ethereum: { proof: '<receipt proof>' } } : 'None';
        const events: EventRecord[] = [{ type: 'DepositObserved', chain, tx_hash: txHash, votes: d.votes }];
        if (status === 'credited') events.push({ type: 'DepositCredited', owner, asset: a.asset, amount });
        if (status === 'held') events.push({ type: 'DepositHeld', owner, asset: a.asset, amount, release_height: d.release_height });
        this.pin(d.last_height, {
          signer: observer,
          kind: 'ObserveDeposit',
          module: 'vaults',
          action: { ObserveDeposit: { chain: CHAIN_ENUM[chain], asset: a.asset, tx_hash: txHash.replace(/^0x/, ''), index: d.index, deposit_index: d.deposit_index, amount, external_height: d.external_height, tip_height: d.external_height + depth, proof } },
          events,
          ok: true,
        });
      }
    }
    this.depositsList.sort((a, b) => b.last_height - a.last_height);
    // Outbounds and batches
    let outId = 1;
    let batchId = 1;
    for (const chain of CHAINS) {
      const assets = ASSETS.filter((a) => a.chain === chain);
      for (let b = 0; b < 6; b++) {
        const created = this.h0 - rng.int(5, 30_000);
        const ids: number[] = [];
        const bStatus = created > this.h0 - 40 ? 'signing' : created > this.h0 - 200 ? 'broadcast' : rng.chance(0.95) ? 'confirmed' : 'failed';
        const txHash = bStatus === 'signing' ? null : chain === 'ETH' ? `0x${rng.hex(32)}` : rng.hex(32);
        for (let i = 0; i < rng.int(1, 6); i++) {
          const a = rng.pick(assets);
          const usd = rng.int(20, 12_000);
          const amount = units(usd / a.usd, a.decimals);
          const nat = ASSET_BY_NAME.get(CHAIN_NATIVE[chain]!)!;
          const fee = units((chain === 'BTC' ? 2.1 : chain === 'ETH' ? 1.4 : 1.0) / nat.usd, nat.decimals);
          const owner = this.user(rng);
          const to = chain === 'BTC' ? `${this.networkId === 'mainnet' ? 'bc1q' : 'tb1q'}${rng.bech32(38)}` : chain === 'ETH' ? `0x${rng.hex(20)}` : `T${rng.base58(33)}`;
          const status: Outbound['status'] = bStatus === 'signing' || bStatus === 'broadcast' ? 'batched' : bStatus === 'failed' ? 'failed' : 'confirmed';
          const queued = created - rng.int(1, 20);
          const o: Outbound = {
            id: outId,
            owner,
            asset: a.asset,
            chain,
            to,
            amount,
            fee,
            status,
            batch_id: batchId,
            tx_hash: txHash,
            queued_height: queued,
            confirmed_height: status === 'confirmed' ? created + rng.int(30, 600) : null,
          };
          this.outboundsList.push(o);
          ids.push(outId);
          this.pin(queued, {
            signer: owner,
            kind: 'Withdraw',
            module: 'vaults',
            action: { Withdraw: { asset: a.asset, to, amount } },
            events: [
              { type: 'Transferred', from: owner, to: `vault:${chain}:outbound`, asset: a.asset, amount },
              { type: 'WithdrawalQueued', outbound_id: outId, owner, asset: a.asset, amount, to },
            ],
            ok: true,
          });
          if (o.confirmed_height && txHash) {
            this.pin(o.confirmed_height, {
              signer: rng.pick(this.observers),
              kind: 'ObserveOutbound',
              module: 'vaults',
              action: { ObserveOutbound: { outbound_id: outId, tx_hash: txHash.replace(/^0x/, ''), external_height: 912_600, tip_height: 912_603, fee_paid: fee, success: true } },
              events: [{ type: 'OutboundConfirmed', outbound_id: outId, tx_hash: txHash }],
              ok: true,
            });
          }
          outId++;
        }
        this.batchesList.push({ id: batchId++, chain, outbound_ids: ids, tx_hash: txHash, fee_paid: txHash ? units(chain === 'BTC' ? 0.00004 : 0.0009, 8) : null, status: bStatus, created_height: created });
      }
    }
    // Queued outbounds waiting for the next batch.
    for (let i = 0; i < 4; i++) {
      const chain = rng.pick(CHAINS);
      const a = rng.pick(ASSETS.filter((x) => x.chain === chain));
      const owner = this.user(rng);
      const amount = units(rng.int(20, 900) / a.usd, a.decimals);
      const to = chain === 'BTC' ? `tb1q${rng.bech32(38)}` : chain === 'ETH' ? `0x${rng.hex(20)}` : `T${rng.base58(33)}`;
      const queued = this.h0 - rng.int(1, 18);
      this.outboundsList.push({ id: outId, owner, asset: a.asset, chain, to, amount, fee: '0', status: 'queued', batch_id: null, tx_hash: null, queued_height: queued, confirmed_height: null });
      this.pin(queued, {
        signer: owner,
        kind: 'Withdraw',
        module: 'vaults',
        action: { Withdraw: { asset: a.asset, to, amount } },
        events: [
          { type: 'Transferred', from: owner, to: `vault:${chain}:outbound`, asset: a.asset, amount },
          { type: 'WithdrawalQueued', outbound_id: outId, owner, asset: a.asset, amount, to },
        ],
        ok: true,
      });
      outId++;
    }
    this.outboundsList.sort((a, b) => b.queued_height - a.queued_height);
    this.batchesList.sort((a, b) => b.created_height - a.created_height);
  }

  // ------------------------------------------------------------ random actions

  private randomAction(rng: Rng, height: number): ActionSpec {
    const signer = this.user(rng);
    const roll = rng.next();
    const fail = (spec: ActionSpec): ActionSpec => {
      if (!rng.chance(0.06)) return spec;
      const error = rng.pick(ERRORS);
      return { ...spec, ok: false, error, events: [] };
    };
    if (roll < 0.34) {
      // PlaceOrder
      const m = rng.pick(MARKETS);
      const base = ASSET_BY_NAME.get(m.base)!;
      const quote = ASSET_BY_NAME.get(m.quote)!;
      const side = rng.chance(0.5) ? 'buy' : 'sell';
      const isMarket = rng.chance(0.2);
      const drift = Math.sin(height / 900) * 0.004;
      const priceWhole = m.price * (1 + drift + (rng.next() - 0.5) * 0.006);
      const price = units(priceWhole, quote.decimals);
      const qty = units(rng.int(5, 4000) / base.usd, base.decimals);
      const orderId = height * 16 + rng.int(0, 15);
      const filledRatio = isMarket ? 1 : rng.pick([0, 0, 0, 0.4, 1]);
      const events: EventRecord[] = [];
      const fills: ActionSpec['fills'] = [];
      const filled = bpsOf(qty, Math.round(filledRatio * 10_000));
      if (filledRatio > 0) {
        const fillQuote = quoteOf(filled, price, base.decimals);
        const fee = side === 'buy' ? bpsOf(filled, 10) : bpsOf(fillQuote, 10);
        const makerId = orderId - rng.int(1, 4000);
        events.push({ type: 'OrderFilled', order_id: orderId, maker_order_id: makerId, pair: m.pair, price, quantity: filled, quote: fillQuote, fee });
        events.push({ type: 'Transferred', from: signer, to: 'system:treasury', asset: side === 'buy' ? m.base : m.quote, amount: bpsOf(fee, 5000) });
        fills.push({ pair: m.pair, fill: { price, quantity: filled, quote: fillQuote, taker: signer, taker_side: side, maker_order_id: makerId, fee } });
      }
      if (filledRatio < 1) {
        events.unshift({ type: 'OrderAccepted', order_id: orderId, owner: signer, pair: m.pair, resting: (BigInt(qty) - BigInt(filled)).toString() });
      }
      return fail({
        signer,
        kind: 'PlaceOrder',
        module: 'markets',
        action: { PlaceOrder: { pair: m.pair, side, order_type: isMarket ? 'market' : 'limit', price: isMarket ? null : price, quantity: isMarket && side === 'buy' ? null : qty, quote_budget: isMarket && side === 'buy' ? quoteOf(qty, price, base.decimals) : null, client_id: rng.chance(0.5) ? rng.int(1, 99_999) : null } },
        events,
        ok: true,
        fills,
      });
    }
    if (roll < 0.46) {
      const orderId = height * 16 - rng.int(16, 9000);
      return fail({ signer, kind: 'CancelOrder', module: 'markets', action: { CancelOrder: { order_id: orderId } }, events: [{ type: 'OrderCancelled', order_id: orderId, released: units(rng.int(10, 3000), 6) }], ok: true });
    }
    if (roll < 0.62) {
      const a = rng.pick(ASSETS);
      const to = this.user(rng);
      const amount = units(rng.int(1, 5000) / a.usd, a.decimals);
      return fail({
        signer,
        kind: 'Transfer',
        module: 'tokens',
        action: { Transfer: { to, asset: a.asset, amount, memo: rng.chance(0.3) ? rng.pick(['invoice 4471', 'thanks', 'refund', 'payroll', 'otc settle']) : null } },
        events: [{ type: 'Transferred', from: signer, to, asset: a.asset, amount }],
        ok: true,
      });
    }
    if (roll < 0.68) {
      const m = rng.pick(MARKETS);
      const quote = ASSET_BY_NAME.get(m.quote)!;
      const base = ASSET_BY_NAME.get(m.base)!;
      const mid = m.price * (1 + Math.sin(height / 900) * 0.004);
      return {
        signer: this.house,
        kind: 'HouseQuote',
        module: 'markets',
        action: { HouseQuote: { pair: m.pair, bid: [units(mid * 0.998, quote.decimals), units(2000 / base.usd, base.decimals)], ask: [units(mid * 1.002, quote.decimals), units(2000 / base.usd, base.decimals)], valid_until: height + 60 } },
        events: [],
        ok: true,
      };
    }
    if (roll < 0.72) {
      const offer = rng.pick(this.offersList);
      const kindRoll = rng.next();
      if (kindRoll < 0.4) {
        return fail({ signer: offer.owner, kind: 'UpdateOffer', module: 'market_p2p', action: { UpdateOffer: { offer_id: offer.id, spec: this.offerSpec(rng, { ...offer, margin_bps: offer.margin_bps + rng.int(-50, 50) }) } }, events: [{ type: 'OfferUpdated', offer_id: offer.id }], ok: true });
      }
      if (kindRoll < 0.8) {
        const paused = rng.chance(0.5);
        return fail({ signer: offer.owner, kind: 'PauseOffer', module: 'market_p2p', action: { PauseOffer: { offer_id: offer.id, paused } }, events: [{ type: 'OfferUpdated', offer_id: offer.id }], ok: true });
      }
      return fail({ signer: offer.owner, kind: 'CloseOffer', module: 'market_p2p', action: { CloseOffer: { offer_id: offer.id } }, events: [{ type: 'OfferClosed', offer_id: offer.id }, { type: 'Transferred', from: `offer:deposit:${offer.id}`, to: offer.owner, asset: 'KEEL', amount: DEFAULT_PARAMS['offer_deposit']! }], ok: true });
    }
    if (roll < 0.75) {
      const trade = rng.pick(this.tradesList);
      return fail({ signer: rng.chance(0.5) ? trade.buyer : trade.seller, kind: 'SubmitEvidence', module: 'disputes', action: { SubmitEvidence: { trade_id: trade.id, evidence_hash: rng.hex(32) } }, events: [], ok: true });
    }
    if (roll < 0.79) {
      const chain = rng.pick(CHAINS);
      const idx = rng.int(1, 9999);
      return fail({ signer, kind: 'RequestDepositAddress', module: 'vaults', action: { RequestDepositAddress: { chain: CHAIN_ENUM[chain] } }, events: [{ type: 'DepositAddressAssigned', owner: signer, chain, index: idx }], ok: true });
    }
    if (roll < 0.82) {
      const chain = rng.pick(CHAINS);
      const obs = rng.pick(this.observers);
      const rate = chain === 'BTC' ? rng.int(8, 40) : chain === 'ETH' ? rng.int(4, 30) : rng.int(300, 600);
      return { signer: obs, kind: 'ReportNetworkFee', module: 'vaults', action: { ReportNetworkFee: { chain: CHAIN_ENUM[chain], fee_rate: rate } }, events: [{ type: 'NetworkFeeReported', chain, observer: obs, fee_rate: rate }], ok: true };
    }
    if (roll < 0.86) {
      const from = rng.pick(['ETH.USDT', 'ETH.USDC', 'TRON.USDT']);
      const amount = units(rng.int(50, 20_000), 6);
      if (rng.chance(0.7)) {
        return fail({ signer, kind: 'MintStable', module: 'stable', action: { MintStable: { asset: from, amount } }, events: [{ type: 'Transferred', from: signer, to: 'system:stable_reserve', asset: from, amount }, { type: 'StableMinted', owner: signer, from, amount }], ok: true });
      }
      return fail({ signer, kind: 'BurnStable', module: 'stable', action: { BurnStable: { asset: from, amount } }, events: [{ type: 'Transferred', from: 'system:stable_reserve', to: signer, asset: from, amount }, { type: 'StableBurned', owner: signer, into: from, amount }], ok: true });
    }
    if (roll < 0.92) {
      const validator = rng.pick(this.validatorsList).address;
      const amount = units(rng.int(100, 50_000), 6);
      const k = rng.next();
      if (k < 0.45) return fail({ signer, kind: 'Delegate', module: 'staking', action: { Delegate: { validator, amount } }, events: [{ type: 'Transferred', from: signer, to: `stake:${validator.slice(0, 8)}`, asset: 'KEEL', amount }, { type: 'Delegated', owner: signer, validator, amount }], ok: true });
      if (k < 0.65) return fail({ signer, kind: 'Undelegate', module: 'staking', action: { Undelegate: { validator, amount } }, events: [{ type: 'Undelegated', owner: signer, validator, amount, at_height: height + 100_000 }], ok: true });
      if (k < 0.85) return fail({ signer, kind: 'ClaimRewards', module: 'staking', action: 'ClaimRewards', events: [{ type: 'RewardsClaimed', owner: signer, asset: 'KUSD', amount: units(rng.int(1, 400) / 7, 6) }, { type: 'RewardsClaimed', owner: signer, asset: 'KEEL', amount: units(rng.int(1, 900), 6) }], ok: true });
      const v = rng.pick(this.validatorsList);
      return fail({ signer: v.address, kind: 'Bond', module: 'staking', action: { Bond: { role: 'Validator', amount, consensus_key: v.consensus_key } }, events: [{ type: 'Bonded', owner: v.address, role: 'validator', amount }], ok: true });
    }
    if (roll < 0.95) {
      const actions = rng.pick([1000, 5000, 10_000]);
      const paid = (BigInt(actions) * BigInt(DEFAULT_PARAMS['budget.price_per_action']!)).toString();
      return fail({ signer, kind: 'BuyBudget', module: 'budgets', action: { BuyBudget: { actions } }, events: [{ type: 'Transferred', from: signer, to: 'system:treasury', asset: 'KEEL', amount: paid }, { type: 'BudgetPurchased', owner: signer, actions, paid }], ok: true });
    }
    if (roll < 0.98) {
      const subject = this.user(rng);
      const tier = rng.pick([1, 2, 3]);
      const attester = rng.pick(this.attesters);
      return { signer: attester, kind: 'Attest', module: 'attest', action: { Attest: { subject, tier, expires_at: Math.floor(this.timestampOf(height) / 1000) + 365 * 86_400 } }, events: [{ type: 'Attested', subject, tier }], ok: true };
    }
    const amount = units(rng.int(100, 5000), 6);
    return fail({ signer, kind: 'Unbond', module: 'staking', action: { Unbond: { role: rng.pick(['Validator', 'Observer', 'Arbitrator']), amount } }, events: [{ type: 'Unbonded', owner: signer, role: 'validator', amount, at_height: height + 100_000 }], ok: true });
  }

  private actionsAt(height: number): ActionSpec[] {
    const cached = this.actionsCache.get(height);
    if (cached) return cached;
    const rng = new Rng(seedOf(this.seed, 'block', height));
    const pinned = this.pinned.get(height) ?? [];
    const n = rng.chance(0.05) ? 0 : rng.int(1, 9) + (rng.chance(0.1) ? rng.int(5, 20) : 0);
    const specs: ActionSpec[] = [...pinned];
    for (let i = 0; i < n; i++) specs.push(this.randomAction(rng, height));
    if (height % 10_000 === 0) {
      specs.push({ signer: this.validatorsList[0]!.address, kind: 'EpochBoundary', module: 'staking', action: 'EpochBoundary', events: [{ type: 'EpochAdvanced', epoch: height / 10_000, validators: 8 }, { type: 'RewardsDistributed', epoch: height / 10_000, asset: 'KUSD', amount: units(12_480.5, 6) }, { type: 'RewardsDistributed', epoch: height / 10_000, asset: 'KEEL', amount: units(64_000, 6) }], ok: true });
    }
    if (this.actionsCache.size > 4000) this.actionsCache.clear();
    this.actionsCache.set(height, specs);
    return specs;
  }

  private stamp(spec: ActionSpec, height: number, index: number): Tx {
    const tx_id = this.txIdAt(height, index);
    this.txIndex.set(tx_id, { height, index });
    const tx: Tx = {
      tx_id,
      height,
      index,
      timestamp: this.timestampOf(height) + index,
      signer: spec.signer,
      module: spec.module,
      kind: spec.kind,
      ok: spec.ok,
      events: spec.events,
    };
    if (spec.error) tx.error = spec.error;
    return tx;
  }

  private blockAt(height: number): BlockDetail {
    const cached = this.blockCache.get(height);
    if (cached) return cached;
    const specs = this.actionsAt(height);
    const receipts = specs.map((s, i) => this.stamp(s, height, i));
    const rng = new Rng(seedOf(this.seed, 'blockhash', height));
    const active = this.validatorsList.filter((v) => !v.jailed);
    const block: BlockDetail = {
      height,
      timestamp: this.timestampOf(height),
      state_hash: rng.hex(32),
      tx_count: receipts.length,
      ok_count: receipts.filter((r) => r.ok).length,
      event_count: receipts.reduce((a, r) => a + r.events.length, 0),
      proposer: active[height % active.length]!.address,
      receipts,
      events: receipts.flatMap((r) => r.events),
    };
    if (this.blockCache.size > 4000) this.blockCache.clear();
    this.blockCache.set(height, block);
    return block;
  }

  private summary(b: BlockDetail): Block {
    const { receipts: _r, events: _e, ...rest } = b;
    return rest;
  }

  private recentTxs(filter: (t: Tx) => boolean, limit: number, fromHeight: number, maxScan = 6000): { txs: Tx[]; next: number | null } {
    const out: Tx[] = [];
    let h = fromHeight;
    let scanned = 0;
    while (h >= 1 && out.length < limit && scanned < maxScan) {
      const b = this.blockAt(h);
      for (let i = b.receipts.length - 1; i >= 0; i--) {
        const t = b.receipts[i]!;
        if (filter(t)) {
          out.push(t);
          if (out.length >= limit) break;
        }
      }
      h--;
      scanned++;
    }
    return { txs: out, next: h >= 1 && out.length >= limit ? h : null };
  }

  private notFound(what: string): never {
    throw new ApiError(404, 'NOT_FOUND', `${what} not found`);
  }

  // ------------------------------------------------------------ ExplorerApi

  health(): Promise<Health> {
    return this.delay(() => {
      const tip = this.tip();
      return { network: this.networkId, chain_id: this.chainId, indexed_height: tip, node_height: tip, lag: 0, state_hash_ok: true, started_at: this.t0 - 86_400_000 * 3 };
    });
  }

  stats(): Promise<Stats> {
    return this.delay(() => {
      const tip = this.tip();
      const rng = new Rng(seedOf(this.seed, 'stats'));
      const assets = ASSETS.map((a) => ({ asset: a.asset, supply: this.supplyOf(a), holders: rng.int(300, 14_000) }));
      const tvl = ASSETS.filter((a) => a.kind === 'vault').reduce((acc, a) => acc + toWhole(this.supplyOf(a), a.decimals) * a.usd, 0);
      return {
        height: tip,
        block_time_ms_avg: this.blockMs + 12,
        tps_1h: 7.4 + Math.sin(tip / 500) * 1.2,
        actions_24h: 1_012_440 + (tip % 5000),
        accounts: 18_432 + Math.floor((tip - this.h0) / 20),
        validators: this.validatorsList.filter((v) => !v.jailed).length,
        tvl_usd: units(tvl, 6),
        assets,
        fees_24h: [
          { asset: 'KUSD', amount: units(4_812.33, 6) },
          { asset: 'BTC.BTC', amount: units(0.0412, 8) },
          { asset: 'ETH.ETH', amount: units(0.91, 18) },
          { asset: 'KEEL', amount: units(9_120, 6) },
        ],
      };
    });
  }

  private supplyOf(a: AssetDef): string {
    if (a.asset === 'KEEL') return units(4_200_000_000, 6);
    if (a.asset === 'KUSD') return units(7_350_000, 6);
    const v = this.vaultsList.find((x) => x.chain === a.chain);
    return v?.liabilities.find((l) => l.asset === a.asset)?.amount ?? '0';
  }

  blocks(q: { limit?: number; cursor?: string } = {}): Promise<BlocksPage> {
    return this.delay(() => {
      const limit = Math.min(q.limit ?? 25, 100);
      const start = q.cursor ? Number(q.cursor) : this.tip();
      const blocks: Block[] = [];
      for (let h = start; h > start - limit && h >= 1; h--) blocks.push(this.summary(this.blockAt(h)));
      const next = start - limit;
      return { blocks, next_cursor: next >= 1 ? String(next) : null };
    });
  }

  block(height: number): Promise<BlockDetail> {
    return this.delay(() => {
      if (!Number.isInteger(height) || height < 1 || height > this.tip()) this.notFound('block');
      return this.blockAt(height);
    });
  }

  txs(q: TxsQuery = {}): Promise<TxsPage> {
    return this.delay(() => {
      const limit = Math.min(q.limit ?? 25, 100);
      const from = q.cursor ? Number(q.cursor) : this.tip();
      const r = this.recentTxs((t) => (q.signer ? t.signer === q.signer : true) && (q.module ? t.module === q.module : true) && (q.ok === undefined ? true : t.ok === q.ok), limit, from, q.signer ? 3000 : 400);
      return { txs: r.txs, next_cursor: r.next ? String(r.next) : null };
    });
  }

  tx(txId: string): Promise<TxDetail> {
    return this.delay(() => {
      const id = txId.toLowerCase().replace(/^0x/, '');
      let loc = this.txIndex.get(id);
      if (!loc) {
        // Materialise recent blocks so a fresh page load can resolve ids.
        const tip = this.tip();
        for (let h = tip; h > tip - 400 && h >= 1 && !loc; h--) {
          this.blockAt(h);
          loc = this.txIndex.get(id);
        }
        if (!loc) {
          for (const h of this.pinned.keys()) {
            this.blockAt(h);
            loc = this.txIndex.get(id);
            if (loc) break;
          }
        }
      }
      if (!loc) this.notFound('transaction');
      const block = this.blockAt(loc.height);
      const tx = block.receipts[loc.index]!;
      const spec = this.actionsAt(loc.height)[loc.index]!;
      return { ...tx, action: spec.action, block: this.summary(block) };
    });
  }

  account(addr: string): Promise<Account> {
    return this.delay(() => {
      const a = addr.toLowerCase();
      if (!/^[0-9a-f]{64}$/.test(a)) this.notFound('account');
      const rng = new Rng(seedOf(this.seed, 'acct', a));
      const known = this.accounts.includes(a);
      const balances = ASSETS.filter(() => known || rng.chance(0.3))
        .filter(() => rng.chance(0.8))
        .flatMap((x) => {
          const avail = units(rng.int(1, 50_000) / x.usd, x.decimals);
          const rows = [{ asset: x.asset, account_type: 'available', balance: avail, decimals: x.decimals }];
          if (rng.chance(0.35)) rows.push({ asset: x.asset, account_type: 'order_lock', balance: bpsOf(avail, rng.int(100, 4000)), decimals: x.decimals });
          if (rng.chance(0.2)) rows.push({ asset: x.asset, account_type: 'escrow', balance: bpsOf(avail, rng.int(50, 900)), decimals: x.decimals });
          return rows;
        });
      const validator = this.validatorsList.find((v) => v.address === a);
      if (validator) balances.push({ asset: 'KEEL', account_type: 'stake_bond', balance: validator.self_bond, decimals: 6 });
      if (this.observers.includes(a)) balances.push({ asset: 'KEEL', account_type: 'observer_bond', balance: units(600_000, 6), decimals: 6 });
      const depositAddresses = CHAINS.filter(() => known || rng.chance(0.5)).map((chain) => ({
        chain,
        index: rng.int(1, 9000),
        address: chain === 'BTC' ? `${this.networkId === 'mainnet' ? 'bc1q' : 'tb1q'}${rng.bech32(38)}` : chain === 'ETH' ? `0x${rng.hex(20)}` : `T${rng.base58(33)}`,
      }));
      const limit = 10_000 + rng.int(0, 250_000);
      const acct: Account = {
        address: a,
        nonce: rng.int(1, 40_000),
        tier: rng.pick([0, 1, 1, 2, 3]),
        budget: { remaining: limit - rng.int(0, 9000), limit, used_this_block: 0 },
        balances,
        first_seen_height: this.heightAt(rng.int(1, 60) * 86_400_000),
        tx_count: rng.int(3, 90_000),
        deposit_addresses: depositAddresses,
        offers_count: this.offersList.filter((o) => o.owner === a).length,
        trades_count: this.tradesList.filter((t) => t.buyer === a || t.seller === a).length,
      };
      if (validator) acct.validator = validator;
      return acct;
    });
  }

  accountTxs(addr: string, q: TxsQuery = {}): Promise<TxsPage> {
    const a = addr.toLowerCase();
    return this.delay(() => {
      const limit = Math.min(q.limit ?? 25, 100);
      const from = q.cursor ? Number(q.cursor) : this.tip();
      const r = this.recentTxs((t) => t.signer === a || t.events.some((e) => e['to'] === a || e['owner'] === a || e['buyer'] === a || e['seller'] === a || e['subject'] === a), limit, from, 2500);
      return { txs: r.txs, next_cursor: r.next ? String(r.next) : null };
    });
  }

  accountTransfers(addr: string, q: { limit?: number; cursor?: string } = {}): Promise<TransfersPage> {
    const a = addr.toLowerCase();
    return this.delay(() => {
      const limit = Math.min(q.limit ?? 25, 100);
      const from = q.cursor ? Number(q.cursor) : this.tip();
      const out: Transfer[] = [];
      let h = from;
      let scanned = 0;
      while (h >= 1 && out.length < limit && scanned < 2500) {
        const b = this.blockAt(h);
        for (const t of b.receipts) {
          for (const e of t.events) {
            const tr = transferOfEvent(e, t);
            if (tr && (tr.from === a || tr.to === a)) out.push(tr);
          }
        }
        h--;
        scanned++;
      }
      return { transfers: out.slice(0, limit), next_cursor: h >= 1 && out.length >= limit ? String(h) : null };
    });
  }

  accountOrders(addr: string): Promise<Order[]> {
    const a = addr.toLowerCase();
    return this.delay(() => this.ordersList.filter((o) => o.owner === a).sort((x, y) => y.created_height - x.created_height));
  }

  accountOffers(addr: string): Promise<Offer[]> {
    const a = addr.toLowerCase();
    return this.delay(() => this.offersList.filter((o) => o.owner === a));
  }

  accountTrades(addr: string): Promise<Trade[]> {
    const a = addr.toLowerCase();
    return this.delay(() => this.tradesList.filter((t) => t.buyer === a || t.seller === a));
  }

  assets(): Promise<Asset[]> {
    return this.delay(() => this.assetRows());
  }

  private assetRows(): Asset[] {
    const rng = new Rng(seedOf(this.seed, 'stats'));
    return ASSETS.map((a) => {
      const row: Asset = { asset: a.asset, decimals: a.decimals, kind: a.kind, supply: this.supplyOf(a), holders: rng.int(300, 14_000) };
      if (a.kind === 'vault') {
        row.chain = a.chain;
        row.reserves = this.vaultsList.find((v) => v.chain === a.chain)?.reserves.find((r) => r.asset === a.asset)?.amount;
      }
      return row;
    });
  }

  asset(asset: string): Promise<AssetDetail> {
    return this.delay(() => {
      const row = this.assetRows().find((a) => a.asset.toLowerCase() === asset.toLowerCase());
      if (!row) this.notFound('asset');
      const rng = new Rng(seedOf(this.seed, 'holders', row.asset));
      let remaining = BigInt(row.supply);
      const holders_top = Array.from({ length: 20 }, (_, i) => {
        const share = i === 0 ? rng.int(800, 2200) : rng.int(50, 600);
        const balance = bpsOf(row.supply, share);
        remaining -= BigInt(balance);
        return { address: this.accounts[(i * 7 + 23) % this.accounts.length]!, balance };
      }).sort((x, y) => (BigInt(y.balance) > BigInt(x.balance) ? 1 : -1));
      return { ...row, holders_top, transfers_24h: rng.int(400, 22_000) };
    });
  }

  private marketRows(): Market[] {
    const tip = this.tip();
    return MARKETS.map((m) => {
      const base = ASSET_BY_NAME.get(m.base)!;
      const quote = ASSET_BY_NAME.get(m.quote)!;
      const candles = this.candleSeries(m, '1h');
      const last = candles.at(-1);
      const lastPrice = last ? Number(BigInt(last.c)) / 10 ** quote.decimals : m.price;
      const dayVolBase = candles.slice(-24).reduce((acc, c) => acc + toWhole(c.v, base.decimals), 0);
      const rng = new Rng(seedOf(this.seed, 'mkt', m.pair, Math.floor(tip / 50)));
      return {
        pair: m.pair,
        base: m.base,
        quote: m.quote,
        base_decimals: base.decimals,
        quote_decimals: quote.decimals,
        last_price: units(lastPrice, quote.decimals),
        volume_24h_base: units(dayVolBase, base.decimals),
        volume_24h_quote: units(dayVolBase * lastPrice, quote.decimals),
        trades_24h: rng.int(800, 9000),
        best_bid: units(lastPrice * (1 - 0.0004), quote.decimals),
        best_ask: units(lastPrice * (1 + 0.0004), quote.decimals),
      };
    });
  }

  markets(): Promise<Market[]> {
    return this.delay(() => this.marketRows());
  }

  market(pair: string): Promise<MarketDetail> {
    return this.delay(() => {
      const row = this.marketRows().find((m) => m.pair.toLowerCase() === pair.toLowerCase());
      if (!row) this.notFound('market');
      const def = MARKETS.find((m) => m.pair === row.pair)!;
      const base = ASSET_BY_NAME.get(def.base)!;
      const quote = ASSET_BY_NAME.get(def.quote)!;
      const rng = new Rng(seedOf(this.seed, 'book', row.pair, Math.floor(this.tip() / 5)));
      const mid = toWhole(row.last_price, quote.decimals);
      const bids: [string, string][] = [];
      const asks: [string, string][] = [];
      let bp = mid * (1 - 0.0004);
      let ap = mid * (1 + 0.0004);
      for (let i = 0; i < 40; i++) {
        bids.push([units(bp, quote.decimals), units((rng.int(50, 2500) * (1 + i / 8)) / base.usd, base.decimals)]);
        asks.push([units(ap, quote.decimals), units((rng.int(50, 2500) * (1 + i / 8)) / base.usd, base.decimals)]);
        bp *= 1 - rng.next() * 0.0012 - 0.0001;
        ap *= 1 + rng.next() * 0.0012 + 0.0001;
      }
      return {
        ...row,
        book: { bids, asks },
        house_quote: { bid: [units(mid * 0.998, quote.decimals), units(2000 / base.usd, base.decimals)], ask: [units(mid * 1.002, quote.decimals), units(2000 / base.usd, base.decimals)], valid_until: this.tip() + 42 },
        taker_fee_bps: 10,
        maker_fee_bps: 0,
      };
    });
  }

  fills(pair: string, q: { limit?: number; cursor?: string } = {}): Promise<FillsPage> {
    return this.delay(() => {
      const p = pair.toLowerCase();
      if (!MARKETS.some((m) => m.pair.toLowerCase() === p)) this.notFound('market');
      const limit = Math.min(q.limit ?? 25, 100);
      const from = q.cursor ? Number(q.cursor) : this.tip();
      const fills: Fill[] = [];
      let h = from;
      let scanned = 0;
      while (h >= 1 && fills.length < limit && scanned < 3000) {
        const specs = this.actionsAt(h);
        const b = this.blockAt(h);
        specs.forEach((s, i) => {
          for (const f of s.fills ?? []) {
            if (f.pair.toLowerCase() === p) {
              const tx = b.receipts[i]!;
              fills.push({ tx_id: tx.tx_id, height: h, timestamp: tx.timestamp, ...f.fill });
            }
          }
        });
        h--;
        scanned++;
      }
      return { fills: fills.slice(0, limit), next_cursor: h >= 1 && fills.length >= limit ? String(h) : null };
    });
  }

  private candleSeries(m: MarketDef, interval: CandleInterval): Candle[] {
    const key = `${m.pair}:${interval}`;
    const cached = this.candleCache.get(key);
    if (cached) return cached;
    const base = ASSET_BY_NAME.get(m.base)!;
    const quote = ASSET_BY_NAME.get(m.quote)!;
    const stepSec = interval === '1m' ? 60 : interval === '5m' ? 300 : interval === '1h' ? 3600 : 86_400;
    const count = interval === '1m' ? 720 : interval === '5m' ? 576 : interval === '1h' ? 720 : 180;
    const rng = new Rng(seedOf(this.seed, 'candles', m.pair, interval));
    const end = Math.floor(this.t0 / 1000 / stepSec) * stepSec;
    const out: Candle[] = [];
    // Walk backwards from the current price so every interval agrees on "now".
    let close = m.price;
    const vol = m.vol * Math.sqrt(stepSec / 3600);
    for (let i = 0; i < count; i++) {
      const t = end - i * stepSec;
      const open = close * (1 + (rng.next() - 0.5) * vol);
      const hi = Math.max(open, close) * (1 + rng.next() * vol * 0.5);
      const lo = Math.min(open, close) * (1 - rng.next() * vol * 0.5);
      const v = (rng.int(200, 20_000) * Math.sqrt(stepSec / 60)) / base.usd;
      out.push({ t, o: units(open, quote.decimals), h: units(hi, quote.decimals), l: units(lo, quote.decimals), c: units(close, quote.decimals), v: units(v, base.decimals) });
      close = open;
    }
    out.reverse();
    this.candleCache.set(key, out);
    return out;
  }

  candles(pair: string, interval: CandleInterval, from?: number, to?: number): Promise<Candle[]> {
    return this.delay(() => {
      const def = MARKETS.find((m) => m.pair.toLowerCase() === pair.toLowerCase());
      if (!def) this.notFound('market');
      return this.candleSeries(def, interval).filter((c) => (from === undefined || c.t >= from) && (to === undefined || c.t <= to));
    });
  }

  order(id: number): Promise<Order> {
    return this.delay(() => this.ordersList.find((o) => o.id === id) ?? this.notFound('order'));
  }

  offers(q: OffersQuery = {}): Promise<OffersPage> {
    return this.delay(() => {
      const offers = this.offersList
        .filter((o) => (q.asset ? o.asset === q.asset : true) && (q.side ? o.side === q.side : true) && (q.status ? o.status === q.status : true))
        .sort((a, b) => b.created_height - a.created_height);
      return { offers, next_cursor: null };
    });
  }

  offer(id: number): Promise<OfferDetail> {
    return this.delay(() => {
      const o = this.offersList.find((x) => x.id === id);
      if (!o) this.notFound('offer');
      return { ...o, trades: this.tradesList.filter((t) => t.offer_id === id) };
    });
  }

  trade(id: number): Promise<Trade> {
    return this.delay(() => this.tradesList.find((t) => t.id === id) ?? this.notFound('trade'));
  }

  validators(): Promise<Validator[]> {
    return this.delay(() => this.validatorsList);
  }

  epochs(): Promise<Epoch[]> {
    return this.delay(() => {
      const cur = Math.floor(this.tip() / 10_000);
      return Array.from({ length: 12 }, (_, i) => ({ epoch: cur - i, start_height: (cur - i) * 10_000, validators: i < 3 ? 8 : i < 7 ? 7 : 5 }));
    });
  }

  vaults(): Promise<Vault[]> {
    return this.delay(() => this.vaultsList);
  }

  vaultDeposits(chain: string, status?: string): Promise<DepositsPage> {
    return this.delay(() => {
      if (!CHAINS.includes(chain as (typeof CHAINS)[number])) this.notFound('vault');
      return { deposits: this.depositsList.filter((d) => d.chain === chain && (status ? d.status === status : true)), next_cursor: null };
    });
  }

  outbounds(status?: string): Promise<OutboundsPage> {
    return this.delay(() => ({
      outbounds: this.outboundsList.filter((o) => (status ? o.status === status : true)),
      batches: this.batchesList,
      next_cursor: null,
    }));
  }

  /** Bitcoin Lightning (2026-09-10): one observer pool and one payout waiting on it. */
  lightning(): Promise<LightningStatus> {
    return this.delay(() => {
      const observer = this.observers[0]!;
      const owner = this.accounts[40] ?? this.accounts[0]!;
      const tip = this.tip();
      return {
        enabled: true,
        params: {
          pool_cap_sats: '5000000',
          max_deposit_sats: '1000000',
          max_withdraw_sats: '1000000',
          max_fee_bps: 50,
          min_fee_sats: '10',
          payout_timeout_blocks: 720,
          daily_cap_sats: '0',
        },
        pool_total: '3026000',
        pools: [
          {
            observer,
            node_id: `02${new Rng(seedOf(this.seed, 'ln-node', 0)).hex(32)}`,
            balance: '3026000',
            available: '3014000',
            pending_out: '12000',
            credited_today: '50000',
            registered_height: Math.max(1, tip - 6_000),
          },
        ],
        assignments: [
          {
            outbound_id: 3,
            observer,
            deadline_height: tip + 650,
            fee_allowance: '60',
            invoice: `lnbc120u1${new Rng(seedOf(this.seed, 'ln-invoice', 3)).hex(48)}`,
            amount: '12000',
            owner,
          },
        ],
        sweeps: [],
      };
    });
  }

  proposals(): Promise<Proposal[]> {
    return this.delay(() => this.proposalsList.map(({ votes: _v, ...p }) => p));
  }

  proposal(id: number): Promise<ProposalDetail> {
    return this.delay(() => this.proposalsList.find((p) => p.id === id) ?? this.notFound('proposal'));
  }

  params(): Promise<Params> {
    return this.delay(() => ({ params: { ...this.paramsNow }, history: this.paramHistory }));
  }

  search(q: string): Promise<SearchResult> {
    return this.delay(() => {
      const s = q.trim();
      const lower = s.toLowerCase();
      const suggestions: NonNullable<SearchResult['suggestions']> = [];
      if (/^\d+$/.test(s)) {
        const n = Number(s);
        if (n >= 1 && n <= this.tip()) return { kind: 'block', ref: s, suggestions: [{ kind: 'offer', ref: s }, { kind: 'trade', ref: s }] };
      }
      const hex = lower.replace(/^0x/, '');
      if (/^[0-9a-f]{64}$/.test(hex)) {
        if (this.txIndex.has(hex)) return { kind: 'tx', ref: hex };
        if (this.validatorsList.some((v) => v.address === hex)) return { kind: 'validator', ref: hex };
        return { kind: 'account', ref: hex };
      }
      const market = MARKETS.find((m) => m.pair.toLowerCase() === lower);
      if (market) return { kind: 'market', ref: market.pair };
      const asset = ASSETS.find((a) => a.asset.toLowerCase() === lower);
      if (asset) return { kind: 'asset', ref: asset.asset, suggestions: MARKETS.filter((m) => m.base === asset.asset).map((m) => ({ kind: 'market', ref: m.pair })) };
      const m2 = /^(offer|trade|block|proposal)\s*#?\s*(\d+)$/.exec(lower);
      if (m2) return { kind: m2[1] as SearchResult['kind'], ref: m2[2]! };
      for (const a of ASSETS) if (a.asset.toLowerCase().includes(lower)) suggestions.push({ kind: 'asset', ref: a.asset });
      for (const m of MARKETS) if (m.pair.toLowerCase().includes(lower)) suggestions.push({ kind: 'market', ref: m.pair });
      return { kind: null, ref: null, suggestions };
    });
  }

  subscribe(onMessage: (msg: WsMessage) => void): () => void {
    this.listeners.add(onMessage);
    if (this.live && !this.timer) {
      this.timer = setInterval(() => {
        const tip = this.tip();
        while (this.lastEmitted < tip) {
          this.lastEmitted++;
          const b = this.blockAt(this.lastEmitted);
          const msg: WsMessage = { type: 'block', block: this.summary(b) };
          for (const l of this.listeners) l(msg);
          for (const tx of b.receipts) for (const l of this.listeners) l({ type: 'tx', tx });
        }
      }, Math.max(200, this.blockMs / 2));
    }
    return () => {
      this.listeners.delete(onMessage);
      if (this.listeners.size === 0 && this.timer) {
        clearInterval(this.timer);
        this.timer = null;
      }
    };
  }
}

/** Projects a ledger-moving event to a transfer row (mirrors the indexer's rule). */
export function transferOfEvent(e: EventRecord, t: Tx): Transfer | null {
  const base = { tx_id: t.tx_id, height: t.height, timestamp: t.timestamp };
  switch (e['type']) {
    case 'Transferred':
      return { ...base, asset: String(e['asset']), amount: String(e['amount']), from: String(e['from']), to: String(e['to']), kind: 'Transferred' };
    case 'DepositCredited':
      return { ...base, asset: String(e['asset']), amount: String(e['amount']), from: 'vault', to: String(e['owner']), kind: 'DepositCredited' };
    case 'TradeReleased':
      return null; // the Transferred legs already carry the amounts
    case 'RewardsClaimed':
      return { ...base, asset: String(e['asset']), amount: String(e['amount']), from: 'system:validator_rewards', to: String(e['owner']), kind: 'RewardsClaimed' };
    default:
      return null;
  }
}

/** Decimals of assets the mock knows; the UI uses this as a fallback only. */
export const MOCK_ASSET_DECIMALS: Record<string, number> = Object.fromEntries(ASSETS.map((a) => [a.asset, a.decimals]));
