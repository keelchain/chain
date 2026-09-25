// HTTP client for the node's /v1 API (see crates/keel-rpc/API.md).
import { Keypair, stringifyJson, txId, type SignedAction } from "./sign.ts";
import type { Action } from "./actions.ts";

export interface Receipt {
  index: number; tx_id: string; signer: string; ok: boolean;
  error: { code: string; message: string } | null; events: unknown[];
}

export class RpcError extends Error {
  readonly status: number;
  readonly body: unknown;

  constructor(status: number, body: unknown) {
    super(`rpc ${status}: ${typeof body === "string" ? body : JSON.stringify(body)}`);
    this.status = status;
    this.body = body;
  }
}

export class RpcClient {
  readonly base: string;
  private readonly fetchImpl: typeof fetch;

  constructor(base = "http://127.0.0.1:5000", fetchImpl: typeof fetch = fetch) {
    this.base = base;
    this.fetchImpl = fetchImpl;
  }

  private async req(method: string, path: string, body?: unknown): Promise<any> {
    const res = await this.fetchImpl(this.base + path, {
      method,
      headers: body === undefined ? {} : { "content-type": "application/json" },
      body: body === undefined ? undefined : stringifyJson(body),
    });
    const text = await res.text();
    let json: unknown = text;
    try { json = JSON.parse(text); } catch { /* keep text */ }
    if (!res.ok) throw new RpcError(res.status, json);
    return json;
  }

  status() { return this.req("GET", "/v1/status"); }
  params() { return this.req("GET", "/v1/params"); }
  account(address: string) { return this.req("GET", `/v1/accounts/${address}`); }
  accountOrders(address: string) { return this.req("GET", `/v1/accounts/${address}/orders`); }
  accountTrades(address: string) { return this.req("GET", `/v1/accounts/${address}/trades`); }
  markets() { return this.req("GET", "/v1/markets"); }
  market(pair: string) { return this.req("GET", `/v1/markets/${pair}`); }
  book(pair: string, depth = 20) { return this.req("GET", `/v1/markets/${pair}/book?depth=${depth}`); }
  order(id: number | bigint) { return this.req("GET", `/v1/orders/${id}`); }
  offers(q: { asset?: string; side?: string; fiat?: string } = {}) {
    const qs = new URLSearchParams(Object.entries(q).filter(([, v]) => v !== undefined) as [string, string][]).toString();
    return this.req("GET", `/v1/offers${qs ? `?${qs}` : ""}`);
  }
  offer(id: number | bigint) { return this.req("GET", `/v1/offers/${id}`); }
  trade(id: number | bigint) { return this.req("GET", `/v1/trades/${id}`); }
  vault(chain: string) { return this.req("GET", `/v1/vaults/${chain}`); }
  vaultAddresses(chain: string, from = 0, limit = 100) { return this.req("GET", `/v1/vaults/${chain}/addresses?from=${from}&limit=${limit}`); }
  outbounds(status?: string) { return this.req("GET", `/v1/vaults/outbounds${status ? `?status=${status}` : ""}`); }
  proposals() { return this.req("GET", "/v1/gov/proposals"); }
  proposal(id: number | bigint) { return this.req("GET", `/v1/gov/proposals/${id}`); }
  validators() { return this.req("GET", "/v1/staking/validators"); }
  receipt(tx: string): Promise<Receipt> { return this.req("GET", `/v1/receipts/${tx}`); }
  blockReceipts(height: number | bigint) { return this.req("GET", `/v1/blocks/${height}/receipts`); }

  /** Submit an already-signed action; resolves to the tx id. */
  async submit(sa: SignedAction): Promise<string> {
    const r = await this.req("POST", "/v1/actions", sa);
    if (!r.admitted) throw new RpcError(422, r);
    return r.tx_id as string;
  }

  /** Fetch the nonce, sign, submit. */
  async send(key: Keypair, action: Action, chainId = 1): Promise<string> {
    const acct = await this.account(key.address);
    const sa = key.sign(BigInt(acct.nonce), chainId, action);
    const id = await this.submit(sa);
    if (id !== txId(sa)) throw new Error("node computed a different tx id");
    return id;
  }

  /** Poll until the receipt exists (or the deadline passes). */
  async waitReceipt(tx: string, timeoutMs = 20_000, everyMs = 250): Promise<Receipt> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      try {
        return await this.receipt(tx);
      } catch (e) {
        if (!(e instanceof RpcError) || e.status !== 404 || Date.now() > deadline) throw e;
      }
      await new Promise((r) => setTimeout(r, everyMs));
    }
  }

  /** WebSocket URL for the per-block feed. */
  wsUrl(): string {
    return this.base.replace(/^http/, "ws") + "/v1/ws";
  }
}
