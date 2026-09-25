/**
 * HTTP implementation of `ExplorerApi` against one indexer base URL, path for
 * path as in docs/explorer-api.md. Errors are surfaced as `ApiError`
 * with the indexer's `{error:{code,message}}` body when present.
 */
import {
  ApiError,
  type Account,
  type Asset,
  type AssetDetail,
  type BlockDetail,
  type BlocksPage,
  type Candle,
  type CandleInterval,
  type DepositsPage,
  type Epoch,
  type ExplorerApi,
  type FillsPage,
  type Health,
  type Market,
  type MarketDetail,
  type Offer,
  type OfferDetail,
  type OffersPage,
  type OffersQuery,
  type Order,
  type OutboundsPage,
  type Params,
  type Proposal,
  type ProposalDetail,
  type SearchResult,
  type Stats,
  type Trade,
  type TransfersPage,
  type TxDetail,
  type TxsPage,
  type TxsQuery,
  type Validator,
  type LightningStatus,
  type Vault,
  type WsMessage,
} from './types';

type QueryValue = string | number | boolean | undefined | null;

export function buildQuery(params: object): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params) as [string, QueryValue][]) {
    if (v === undefined || v === null || v === '') continue;
    sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : '';
}

export function wsUrl(baseUrl: string): string {
  const u = new URL('/v1/ws', baseUrl);
  u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:';
  return u.toString();
}

export class HttpExplorerApi implements ExplorerApi {
  readonly baseUrl: string;
  private readonly fetchFn: typeof fetch;

  constructor(baseUrl: string, fetchFn: typeof fetch = (...args) => fetch(...args)) {
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.fetchFn = fetchFn;
  }

  private async get<T>(path: string): Promise<T> {
    let res: Response;
    try {
      res = await this.fetchFn(`${this.baseUrl}${path}`, {
        headers: { accept: 'application/json' },
      });
    } catch (e) {
      throw new ApiError(0, 'NETWORK', e instanceof Error ? e.message : 'network error');
    }
    if (!res.ok) {
      let code = `HTTP_${res.status}`;
      let message = res.statusText || 'request failed';
      try {
        const body = (await res.json()) as { error?: { code?: string; message?: string } };
        if (body?.error) {
          code = body.error.code ?? code;
          message = body.error.message ?? message;
        }
      } catch {
        // non-JSON error body
      }
      throw new ApiError(res.status, code, message);
    }
    return (await res.json()) as T;
  }

  health() {
    return this.get<Health>('/v1/health');
  }
  stats() {
    return this.get<Stats>('/v1/stats');
  }
  blocks(q: { limit?: number; cursor?: string } = {}) {
    return this.get<BlocksPage>(`/v1/blocks${buildQuery(q)}`);
  }
  block(height: number) {
    return this.get<BlockDetail>(`/v1/blocks/${height}`);
  }
  txs(q: TxsQuery = {}) {
    return this.get<TxsPage>(`/v1/txs${buildQuery(q)}`);
  }
  tx(txId: string) {
    return this.get<TxDetail>(`/v1/txs/${encodeURIComponent(txId)}`);
  }
  account(addr: string) {
    return this.get<Account>(`/v1/accounts/${encodeURIComponent(addr)}`);
  }
  accountTxs(addr: string, q: TxsQuery = {}) {
    return this.get<TxsPage>(`/v1/accounts/${encodeURIComponent(addr)}/txs${buildQuery(q)}`);
  }
  accountTransfers(addr: string, q: { limit?: number; cursor?: string } = {}) {
    return this.get<TransfersPage>(`/v1/accounts/${encodeURIComponent(addr)}/transfers${buildQuery(q)}`);
  }
  accountOrders(addr: string) {
    return this.get<{ orders: Order[] } | Order[]>(`/v1/accounts/${encodeURIComponent(addr)}/orders`).then(unwrap('orders'));
  }
  accountOffers(addr: string) {
    return this.get<{ offers: Offer[] } | Offer[]>(`/v1/accounts/${encodeURIComponent(addr)}/offers`).then(unwrap('offers'));
  }
  accountTrades(addr: string) {
    return this.get<{ trades: Trade[] } | Trade[]>(`/v1/accounts/${encodeURIComponent(addr)}/trades`).then(unwrap('trades'));
  }
  assets() {
    return this.get<Asset[]>('/v1/assets');
  }
  asset(asset: string) {
    return this.get<AssetDetail>(`/v1/assets/${encodeURIComponent(asset)}`);
  }
  markets() {
    return this.get<Market[]>('/v1/markets');
  }
  market(pair: string) {
    return this.get<MarketDetail>(`/v1/markets/${encodeURIComponent(pair)}`);
  }
  fills(pair: string, q: { limit?: number; cursor?: string } = {}) {
    return this.get<FillsPage>(`/v1/markets/${encodeURIComponent(pair)}/fills${buildQuery(q)}`);
  }
  candles(pair: string, interval: CandleInterval, from?: number, to?: number) {
    return this.get<Candle[]>(`/v1/markets/${encodeURIComponent(pair)}/candles${buildQuery({ interval, from, to })}`);
  }
  order(id: number) {
    return this.get<Order>(`/v1/orders/${id}`);
  }
  offers(q: OffersQuery = {}) {
    return this.get<OffersPage>(`/v1/offers${buildQuery(q)}`);
  }
  offer(id: number) {
    return this.get<OfferDetail>(`/v1/offers/${id}`);
  }
  trade(id: number) {
    return this.get<Trade>(`/v1/trades/${id}`);
  }
  validators() {
    return this.get<Validator[]>('/v1/validators');
  }
  epochs() {
    return this.get<Epoch[]>('/v1/epochs');
  }
  vaults() {
    return this.get<Vault[]>('/v1/vaults');
  }
  vaultDeposits(chain: string, status?: string) {
    return this.get<DepositsPage>(`/v1/vaults/${encodeURIComponent(chain)}/deposits${buildQuery({ status })}`);
  }
  outbounds(status?: string) {
    return this.get<OutboundsPage>(`/v1/vaults/outbounds${buildQuery({ status })}`);
  }
  lightning() {
    return this.get<LightningStatus>('/v1/lightning');
  }
  proposals() {
    return this.get<Proposal[]>('/v1/governance/proposals');
  }
  proposal(id: number) {
    return this.get<ProposalDetail>(`/v1/governance/proposals/${id}`);
  }
  params() {
    return this.get<Params>('/v1/governance/params');
  }
  search(q: string) {
    return this.get<SearchResult>(`/v1/search${buildQuery({ q })}`);
  }

  subscribe(onMessage: (msg: WsMessage) => void): () => void {
    let closed = false;
    let ws: WebSocket | null = null;
    let pollTimer: ReturnType<typeof setInterval> | null = null;
    let lastHeight = -1;

    const startPolling = () => {
      if (pollTimer || closed) return;
      const poll = async () => {
        try {
          const page = await this.blocks({ limit: 1 });
          const b = page.blocks[0];
          if (b && b.height !== lastHeight) {
            lastHeight = b.height;
            onMessage({ type: 'block', block: b });
          }
        } catch {
          // indexer unreachable; keep trying quietly
        }
      };
      void poll();
      pollTimer = setInterval(() => void poll(), 3000);
    };

    if (typeof WebSocket === 'undefined') {
      startPolling();
    } else {
      try {
        ws = new WebSocket(wsUrl(this.baseUrl));
        ws.onmessage = (ev) => {
          try {
            const msg = JSON.parse(String(ev.data)) as WsMessage;
            if (msg.type === 'block') lastHeight = msg.block.height;
            onMessage(msg);
          } catch {
            // ignore malformed frame
          }
        };
        ws.onerror = () => startPolling();
        ws.onclose = () => startPolling();
      } catch {
        startPolling();
      }
    }

    return () => {
      closed = true;
      if (pollTimer) clearInterval(pollTimer);
      if (ws) {
        ws.onclose = null;
        ws.onerror = null;
        try {
          ws.close();
        } catch {
          // already closed
        }
      }
    };
  }
}

function unwrap<K extends string, T>(key: K): (v: Record<K, T[]> | T[]) => T[] {
  return (v) => (Array.isArray(v) ? v : v[key]);
}
