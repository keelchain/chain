/**
 * Wire types of the indexer API (docs/explorer-api.md, v1).
 *
 * Amounts are decimal strings in smallest units; where an asset is involved a
 * `decimals` accompanies it. Addresses and hashes are hex. Lists are newest
 * first and page with `?limit=&cursor=`.
 */

export type Hex = string;
export type AmountStr = string;

export interface Health {
  network: string;
  chain_id: string;
  indexed_height: number;
  node_height: number;
  lag: number;
  state_hash_ok: boolean;
  started_at: number;
}

export interface Stats {
  height: number;
  block_time_ms_avg: number;
  tps_1h: number;
  actions_24h: number;
  accounts: number;
  validators: number;
  tvl_usd: AmountStr;
  assets: { asset: string; supply: AmountStr; holders: number }[];
  fees_24h: { asset: string; amount: AmountStr }[];
}

export interface Block {
  height: number;
  timestamp: number;
  state_hash: Hex;
  tx_count: number;
  ok_count: number;
  event_count: number;
  proposer?: Hex;
}

export type EventRecord = { type: string } & Record<string, unknown>;

export interface TxError {
  code: string;
  message: string;
}

export interface Tx {
  tx_id: Hex;
  height: number;
  index: number;
  timestamp: number;
  signer: Hex;
  module: string;
  kind: string;
  ok: boolean;
  error?: TxError;
  events: EventRecord[];
}

/** Decoded action as serde JSON of `keel_actions::Action` (externally tagged). */
export type DecodedAction = Record<string, unknown> | string;

export interface TxDetail extends Tx {
  action: DecodedAction;
  block: Block;
}

export interface BlockDetail extends Block {
  receipts: Tx[];
  events: EventRecord[];
}

export interface BlocksPage {
  blocks: Block[];
  next_cursor?: string | null;
}

export interface TxsPage {
  txs: Tx[];
  next_cursor?: string | null;
}

export interface TxsQuery {
  limit?: number;
  cursor?: string;
  signer?: string;
  module?: string;
  ok?: boolean;
}

export interface Balance {
  asset: string;
  account_type: string;
  balance: AmountStr;
  decimals: number;
}

export interface DepositAddress {
  chain: string;
  index: number;
  address: string;
}

export interface Account {
  address: Hex;
  nonce: number;
  tier: number;
  budget: { remaining: number; limit: number; used_this_block?: number } | number;
  balances: Balance[];
  first_seen_height: number;
  tx_count: number;
  deposit_addresses: DepositAddress[];
  validator?: Validator;
  offers_count: number;
  trades_count: number;
}

export type TransferKind =
  | 'Transferred'
  | 'DepositCredited'
  | 'OutboundConfirmed'
  | 'OrderFilled'
  | 'TradeReleased'
  | string;

export interface Transfer {
  tx_id: Hex;
  height: number;
  timestamp: number;
  asset: string;
  amount: AmountStr;
  decimals?: number;
  from: Hex | string;
  to: Hex | string;
  kind: TransferKind;
}

export interface TransfersPage {
  transfers: Transfer[];
  next_cursor?: string | null;
}

export type AssetKind = 'native' | 'stable' | 'vault';

export interface Asset {
  asset: string;
  decimals: number;
  kind: AssetKind;
  supply: AmountStr;
  holders: number;
  reserves?: AmountStr;
  chain?: string;
}

export interface AssetDetail extends Asset {
  holders_top: { address: Hex; balance: AmountStr }[];
  transfers_24h: number;
}

export interface Market {
  pair: string;
  base: string;
  quote: string;
  base_decimals?: number;
  quote_decimals?: number;
  last_price: AmountStr;
  volume_24h_base: AmountStr;
  volume_24h_quote: AmountStr;
  trades_24h: number;
  best_bid: AmountStr | null;
  best_ask: AmountStr | null;
}

export type BookLevel = [AmountStr, AmountStr];

export interface MarketDetail extends Market {
  book: { bids: BookLevel[]; asks: BookLevel[] };
  house_quote?: { bid?: BookLevel | null; ask?: BookLevel | null; valid_until?: number } | null;
  taker_fee_bps?: number;
  maker_fee_bps?: number;
}

export interface Fill {
  tx_id: Hex;
  height: number;
  timestamp: number;
  price: AmountStr;
  quantity: AmountStr;
  quote: AmountStr;
  taker: Hex;
  taker_side?: 'buy' | 'sell';
  maker_order_id: number | null;
  fee: AmountStr;
}

export interface FillsPage {
  fills: Fill[];
  next_cursor?: string | null;
}

export type CandleInterval = '1m' | '5m' | '1h' | '1d';

export interface Candle {
  t: number;
  o: AmountStr;
  h: AmountStr;
  l: AmountStr;
  c: AmountStr;
  v: AmountStr;
}

export interface Order {
  id: number;
  owner: Hex;
  pair: string;
  side: 'buy' | 'sell';
  order_type: 'limit' | 'market';
  price: AmountStr | null;
  quantity: AmountStr;
  filled: AmountStr;
  status: string;
  created_height: number;
  tx_id: Hex;
  fills: Fill[];
}

export type OfferStatus = 'active' | 'paused' | 'closed' | string;

export interface Offer {
  id: number;
  owner: Hex;
  side: 'buy' | 'sell';
  asset: string;
  fiat_currency: string;
  payment_method: string;
  margin_bps: number;
  min_amount: AmountStr;
  max_amount: AmountStr;
  payment_window_secs: number;
  country: string | null;
  min_tier: number;
  status: OfferStatus;
  created_height: number;
}

export interface OffersQuery {
  asset?: string;
  side?: 'buy' | 'sell';
  status?: string;
  limit?: number;
  cursor?: string;
}

export interface OffersPage {
  offers: Offer[];
  next_cursor?: string | null;
}

export interface OfferDetail extends Offer {
  trades: Trade[];
}

export type TradeStatus =
  | 'funded'
  | 'paid'
  | 'released'
  | 'cancelled'
  | 'disputed'
  | 'ruled'
  | string;

export interface TradeHistoryEntry {
  status: TradeStatus;
  height: number;
  timestamp: number;
  tx_id?: Hex;
  by?: Hex;
}

export interface Dispute {
  opened_by: Hex;
  opened_height: number;
  evidence: { by: Hex; hash: Hex; height: number }[];
  ruling?: { kind: string; buyer_bps?: number; buyer_amount: AmountStr; seller_amount: AmountStr; height: number; arbitrator?: Hex } | null;
}

export interface Trade {
  id: number;
  offer_id: number;
  buyer: Hex;
  seller: Hex;
  asset: string;
  amount: AmountStr;
  fee: AmountStr;
  fiat_amount: AmountStr;
  fiat_currency: string;
  status: TradeStatus;
  started_height: number;
  deadline: number;
  paid_at?: number | null;
  closed_height?: number | null;
  dispute?: Dispute | null;
  history?: TradeHistoryEntry[];
}

export interface Validator {
  address: Hex;
  consensus_key: Hex;
  self_bond: AmountStr;
  delegated: AmountStr;
  power: AmountStr;
  jailed: boolean;
  blocks_proposed_24h?: number;
  uptime?: number;
}

export interface Epoch {
  epoch: number;
  start_height: number;
  validators: number;
}

export interface Vault {
  chain: string;
  epoch: number;
  address_count: number;
  reserves: { asset: string; amount: AmountStr }[];
  liabilities: { asset: string; amount: AmountStr }[];
  fee_rate: number;
  halted: boolean;
  threshold?: number;
  signers?: Hex[];
}

export type DepositStatus = 'pending' | 'credited' | 'held' | 'rejected';

export interface Deposit {
  chain: string;
  asset: string;
  tx_hash: string;
  index: number;
  owner: Hex;
  deposit_index: number;
  amount: AmountStr;
  external_height: number;
  depth: number;
  required_depth: number;
  votes: number;
  status: DepositStatus;
  first_height: number;
  last_height: number;
  release_height?: number | null;
}

export interface DepositsPage {
  deposits: Deposit[];
  next_cursor?: string | null;
}

export type OutboundStatus = 'queued' | 'batched' | 'confirmed' | 'failed';

export interface Outbound {
  id: number;
  owner: Hex;
  asset: string;
  chain: string;
  to: string;
  amount: AmountStr;
  fee: AmountStr;
  status: OutboundStatus;
  batch_id?: number | null;
  tx_hash?: string | null;
  queued_height: number;
  confirmed_height?: number | null;
}

export interface OutboundBatch {
  id: number;
  chain: string;
  outbound_ids: number[];
  tx_hash?: string | null;
  fee_paid?: AmountStr | null;
  status: 'signing' | 'broadcast' | 'confirmed' | 'failed' | string;
  created_height: number;
}

export interface OutboundsPage {
  outbounds: Outbound[];
  batches: OutboundBatch[];
  next_cursor?: string | null;
}

/* ---- Bitcoin Lightning (2026-09-10) ---------------------------- */

/** The chain's Lightning knobs as the node serves them (satoshi strings, bps, blocks). */
export interface LightningParams {
  pool_cap_sats: AmountStr;
  max_deposit_sats: AmountStr;
  max_withdraw_sats: AmountStr;
  max_fee_bps: number;
  min_fee_sats: AmountStr;
  payout_timeout_blocks: number;
  daily_cap_sats: AmountStr;
}

/** One observer's Lightning pool: BTC its node holds on the chain's behalf. */
export interface LightningPool {
  observer: Hex;
  node_id: string;
  balance: AmountStr;
  available: AmountStr;
  pending_out: AmountStr;
  credited_today: AmountStr;
  registered_height: number;
}

/** A payout assigned to an observer, refunded to the owner if unpaid by the deadline. */
export interface LightningAssignment {
  outbound_id: number;
  observer: Hex;
  deadline_height: number;
  fee_allowance: AmountStr;
  invoice: string;
  amount: AmountStr;
  owner: Hex;
}

/** Excess over the pool cap on its way back to the on-chain vault. */
export interface LightningSweep {
  tx_hash: string;
  observer: Hex;
  amount: AmountStr;
}

export interface LightningStatus {
  enabled: boolean;
  params: LightningParams;
  pool_total: AmountStr;
  pools: LightningPool[];
  assignments: LightningAssignment[];
  sweeps: LightningSweep[];
}

export type ProposalStatus = 'voting' | 'passed' | 'rejected' | 'vetoed' | 'executed' | 'failed' | string;

export interface Tally {
  yes: AmountStr;
  no: AmountStr;
  abstain: AmountStr;
  veto: AmountStr;
  total_bonded?: AmountStr;
}

export interface Proposal {
  id: number;
  proposer: Hex;
  title: string;
  description: string;
  kind: Record<string, unknown> | string;
  status: ProposalStatus;
  deposit: AmountStr;
  submitted_height: number;
  voting_end_height: number;
  execute_height?: number | null;
  tally: Tally;
}

export interface Vote {
  voter: Hex;
  choice: 'yes' | 'no' | 'abstain' | 'veto';
  weight: AmountStr;
  height: number;
  tx_id: Hex;
}

export interface ProposalDetail extends Proposal {
  votes: Vote[];
}

export interface ParamChange {
  height: number;
  key: string;
  from: AmountStr;
  to: AmountStr;
  tx_id: Hex;
}

export interface Params {
  params: Record<string, AmountStr>;
  history: ParamChange[];
}

export type SearchKind = 'block' | 'tx' | 'account' | 'asset' | 'market' | 'offer' | 'trade' | 'validator';

export interface SearchResult {
  kind: SearchKind | null;
  ref: string | null;
  suggestions?: { kind: SearchKind; ref: string; label?: string }[];
}

export type WsMessage =
  | { type: 'block'; block: Block & { tx_count: number } }
  | { type: 'tx'; tx: Tx };

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

/** Everything the UI can ask an indexer. One instance per network. */
export interface ExplorerApi {
  readonly baseUrl: string;
  health(): Promise<Health>;
  stats(): Promise<Stats>;
  blocks(q?: { limit?: number; cursor?: string }): Promise<BlocksPage>;
  block(height: number): Promise<BlockDetail>;
  txs(q?: TxsQuery): Promise<TxsPage>;
  tx(txId: string): Promise<TxDetail>;
  account(addr: string): Promise<Account>;
  accountTxs(addr: string, q?: TxsQuery): Promise<TxsPage>;
  accountTransfers(addr: string, q?: { limit?: number; cursor?: string }): Promise<TransfersPage>;
  accountOrders(addr: string): Promise<Order[]>;
  accountOffers(addr: string): Promise<Offer[]>;
  accountTrades(addr: string): Promise<Trade[]>;
  assets(): Promise<Asset[]>;
  asset(asset: string): Promise<AssetDetail>;
  markets(): Promise<Market[]>;
  market(pair: string): Promise<MarketDetail>;
  fills(pair: string, q?: { limit?: number; cursor?: string }): Promise<FillsPage>;
  candles(pair: string, interval: CandleInterval, from?: number, to?: number): Promise<Candle[]>;
  order(id: number): Promise<Order>;
  offers(q?: OffersQuery): Promise<OffersPage>;
  offer(id: number): Promise<OfferDetail>;
  trade(id: number): Promise<Trade>;
  validators(): Promise<Validator[]>;
  epochs(): Promise<Epoch[]>;
  vaults(): Promise<Vault[]>;
  vaultDeposits(chain: string, status?: string): Promise<DepositsPage>;
  outbounds(status?: string): Promise<OutboundsPage>;
  lightning(): Promise<LightningStatus>;
  proposals(): Promise<Proposal[]>;
  proposal(id: number): Promise<ProposalDetail>;
  params(): Promise<Params>;
  search(q: string): Promise<SearchResult>;
  /** Live feed: resolves to an unsubscribe function. Falls back to polling. */
  subscribe(onMessage: (msg: WsMessage) => void): () => void;
}
