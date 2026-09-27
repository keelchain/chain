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

/** `GET /v1/ready/{chain}`. */
export interface Readiness {
  chain: string;
  ready: boolean;
  reasons: string[];
  height: number;
  vault: { epoch: number; public_key: string; signers: string[]; threshold: number } | null;
  checkpoint: Record<string, unknown> | null;
  last_deposit_credited: { height: number; timestamp: number } | null;
  outbound_pending: number;
  halted: string[];
  fee_rate: number;
}

/** Minimal indexer lookup used by `waitReceipt`'s fallback. */
class IndexerLookup {
  private readonly base: string;
  private readonly fetchImpl: typeof fetch;
  constructor(base: string, fetchImpl: typeof fetch) {
    this.base = base;
    this.fetchImpl = fetchImpl;
  }
  async tx(id: string): Promise<any> {
    const res = await this.fetchImpl(`${this.base.replace(/\/$/, "")}/v1/txs/${id}`);
    const text = await res.text();
    let json: unknown = text;
    try { json = JSON.parse(text); } catch { /* keep text */ }
    if (!res.ok) throw new RpcError(res.status, json);
    return json;
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
  vaultAddresses(chain: string, from = 0, limit = 100, owner?: string) {
    return this.req("GET", `/v1/vaults/${chain}/addresses?from=${from}&limit=${limit}${owner ? `&owner=${owner}` : ""}`);
  }
  /** The account's deposit address on `chain` ("BTC" | "ETH" | "TRON"), or null until one is requested. */
  async depositAddress(chain: string, owner: string): Promise<{ index: number; address: string } | null> {
    const r = await this.vaultAddresses(chain, 0, 1, owner);
    const row = r.addresses?.[0];
    return row ? { index: row.index, address: row.address } : null;
  }
  /** Request a deposit address on `chain` for the key's account and wait for it. */
  async requestDepositAddress(key: Keypair, chain: "BTC" | "ETH" | "TRON", chainId = 1): Promise<{ index: number; address: string }> {
    const have = await this.depositAddress(chain, key.address);
    if (have) return have;
    const name = ({ BTC: "Bitcoin", ETH: "Ethereum", TRON: "Tron" } as const)[chain];
    const tx = await this.send(key, { RequestDepositAddress: { chain: name } }, chainId);
    const receipt = await this.waitReceipt(tx);
    if (!receipt.ok) throw new RpcError(422, receipt.error);
    const got = await this.depositAddress(chain, key.address);
    if (!got) throw new Error("deposit address not assigned");
    return got;
  }
  outbounds(status?: string) { return this.req("GET", `/v1/vaults/outbounds${status ? `?status=${status}` : ""}`); }
  proposals() { return this.req("GET", "/v1/gov/proposals"); }
  proposal(id: number | bigint) { return this.req("GET", `/v1/gov/proposals/${id}`); }
  validators() { return this.req("GET", "/v1/staking/validators"); }
  receipt(tx: string): Promise<Receipt> { return this.req("GET", `/v1/receipts/${tx}`); }
  blockReceipts(height: number | bigint) { return this.req("GET", `/v1/blocks/${height}/receipts`); }
  blocks(before?: number | bigint, limit = 50) { return this.req("GET", `/v1/blocks?limit=${limit}${before !== undefined ? `&before=${before}` : ""}`); }
  block(height: number | bigint) { return this.req("GET", `/v1/blocks/${height}`); }
  blockActions(height: number | bigint) { return this.req("GET", `/v1/blocks/${height}/actions`); }
  lightning() { return this.req("GET", "/v1/lightning"); }
  /** Deposits the node tracks (pending, credited, held, rejected). */
  deposits(q: { status?: string; owner?: string } = {}) {
    const qs = new URLSearchParams(Object.entries(q).filter(([, v]) => v !== undefined) as [string, string][]).toString();
    return this.req("GET", `/v1/vaults/deposits${qs ? `?${qs}` : ""}`);
  }
  /** The address string of an assigned deposit index. */
  vaultAddress(chain: string, index: number | bigint) { return this.req("GET", `/v1/vaults/${chain}/addresses/${index}`); }
  /** Reverse lookup of a deposit address. */
  lookupAddress(chain: string, address: string) { return this.req("GET", `/v1/vaults/${chain}/addresses/lookup?address=${encodeURIComponent(address)}`); }
  roles() { return this.req("GET", "/v1/gov/roles"); }
  /** Whether a client may turn `chain` on: vault, checkpoint age, halts, pending outbounds, reasons. */
  ready(chain: "BTC" | "ETH" | "TRON", maxCheckpointAgeSecs?: number): Promise<Readiness> {
    return this.req("GET", `/v1/ready/${chain}${maxCheckpointAgeSecs !== undefined ? `?max_checkpoint_age=${maxCheckpointAgeSecs}` : ""}`);
  }
  /** Newest snapshot metadata for state sync (`keel-node --sync-from`). */
  syncMeta() { return this.req("GET", "/v1/sync/meta"); }

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

  /**
   * Poll until the receipt exists (or the deadline passes). The node keeps
   * receipts for its recent blocks only; pass an indexer base URL (or an
   * `IndexerClient`) to fall back to the full history at `/v1/txs/{id}`.
   */
  async waitReceipt(tx: string, timeoutMs = 20_000, everyMs = 250, indexer?: string | { tx(id: string): Promise<any> }): Promise<Receipt> {
    const deadline = Date.now() + timeoutMs;
    const fallback = typeof indexer === "string" ? new IndexerLookup(indexer, this.fetchImpl) : indexer;
    for (;;) {
      try {
        return await this.receipt(tx);
      } catch (e) {
        if (!(e instanceof RpcError) || e.status !== 404) throw e;
        if (fallback) {
          try {
            const t = await fallback.tx(tx);
            if (t && typeof t === "object" && "ok" in t) {
              return { index: t.index ?? 0, tx_id: tx, signer: t.signer, ok: Boolean(t.ok), error: t.error ?? null, events: t.events ?? [] };
            }
          } catch (e2) {
            if (!(e2 instanceof RpcError) || e2.status !== 404) throw e2;
          }
        }
        if (Date.now() > deadline) throw e;
      }
      await new Promise((r) => setTimeout(r, everyMs));
    }
  }

  /** WebSocket URL for the per-block feed. */
  wsUrl(): string {
    return this.base.replace(/^http/, "ws") + "/v1/ws";
  }
}
