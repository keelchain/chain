// HTTP client for the indexer's read API (docs/explorer-api.md): history,
// candles, per-account pages, deposits and the search box. The node RPC
// (client.ts) answers about the present; this answers about the past.
import { RpcError } from "./client.ts";

export interface Page<T> { items: T[]; next_cursor: string | null }

export class IndexerClient {
  readonly base: string;
  private readonly fetchImpl: typeof fetch;

  constructor(base = "https://testnet.keelchain.com/api", fetchImpl: typeof fetch = fetch) {
    this.base = base.replace(/\/$/, "");
    this.fetchImpl = fetchImpl;
  }

  private async get(path: string, query: Record<string, string | number | bigint | boolean | undefined> = {}): Promise<any> {
    const qs = new URLSearchParams(
      Object.entries(query).filter(([, v]) => v !== undefined).map(([k, v]) => [k, String(v)]),
    ).toString();
    const res = await this.fetchImpl(`${this.base}${path}${qs ? `?${qs}` : ""}`);
    const text = await res.text();
    let json: unknown = text;
    try { json = JSON.parse(text); } catch { /* keep text */ }
    if (!res.ok) throw new RpcError(res.status, json);
    return json;
  }

  health() { return this.get("/v1/health"); }
  stats() { return this.get("/v1/stats"); }
  blocks(q: { limit?: number; cursor?: string } = {}) { return this.get("/v1/blocks", q); }
  block(height: number | bigint) { return this.get(`/v1/blocks/${height}`); }
  txs(q: { limit?: number; cursor?: string; signer?: string; module?: string; ok?: boolean } = {}) { return this.get("/v1/txs", q); }
  tx(txId: string) { return this.get(`/v1/txs/${txId}`); }
  account(address: string) { return this.get(`/v1/accounts/${address}`); }
  accountTxs(address: string, q: { limit?: number; cursor?: string } = {}) { return this.get(`/v1/accounts/${address}/txs`, q); }
  accountOrders(address: string, q: { limit?: number; cursor?: string } = {}) { return this.get(`/v1/accounts/${address}/orders`, q); }
  accountOffers(address: string, q: { limit?: number; cursor?: string } = {}) { return this.get(`/v1/accounts/${address}/offers`, q); }
  accountTrades(address: string, q: { limit?: number; cursor?: string } = {}) { return this.get(`/v1/accounts/${address}/trades`, q); }
  accountTransfers(address: string, q: { limit?: number; cursor?: string } = {}) { return this.get(`/v1/accounts/${address}/transfers`, q); }
  assets() { return this.get("/v1/assets"); }
  asset(asset: string) { return this.get(`/v1/assets/${asset}`); }
  markets() { return this.get("/v1/markets"); }
  market(pair: string) { return this.get(`/v1/markets/${pair}`); }
  fills(pair: string, q: { limit?: number; cursor?: string } = {}) { return this.get(`/v1/markets/${pair}/fills`, q); }
  candles(pair: string, q: { interval?: "1m" | "5m" | "1h" | "1d"; from?: number; to?: number } = {}) { return this.get(`/v1/markets/${pair}/candles`, q); }
  order(id: number | bigint) { return this.get(`/v1/orders/${id}`); }
  offers(q: { asset?: string; side?: string; status?: string; owner?: string; limit?: number; cursor?: string } = {}) { return this.get("/v1/offers", q); }
  offer(id: number | bigint) { return this.get(`/v1/offers/${id}`); }
  trade(id: number | bigint) { return this.get(`/v1/trades/${id}`); }
  validators() { return this.get("/v1/validators"); }
  epochs() { return this.get("/v1/epochs"); }
  vaults() { return this.get("/v1/vaults"); }
  deposits(chain: string, q: { status?: string; owner?: string; limit?: number; cursor?: string } = {}) { return this.get(`/v1/vaults/${chain}/deposits`, q); }
  outbounds(q: { status?: string; owner?: string; limit?: number; cursor?: string } = {}) { return this.get("/v1/vaults/outbounds", q); }
  proposals() { return this.get("/v1/governance/proposals"); }
  proposal(id: number | bigint) { return this.get(`/v1/governance/proposals/${id}`); }
  governanceParams() { return this.get("/v1/governance/params"); }
  search(q: string) { return this.get("/v1/search", { q }); }

  /** `ws(s)://…/v1/ws` for `KeelSocket`. */
  wsUrl(): string { return `${this.base.replace(/^http/, "ws")}/v1/ws`; }
}
