/**
 * HTTP implementation of `HarborApi`: the node RPC for state, the indexer
 * for history. Errors surface as `ApiError` with the server's
 * `{error:{code,message}}` body when present.
 */
import { ApiError, type AccountRecord, type Book, type ClientRecord, type Custody, type HarborApi, type HistoryPage } from './types';

export class HttpHarborApi implements HarborApi {
  readonly rpcUrl: string;
  readonly indexerUrl: string;
  private readonly fetchFn: typeof fetch;

  constructor(rpcUrl: string, indexerUrl: string, fetchFn: typeof fetch = (...args) => fetch(...args)) {
    this.rpcUrl = rpcUrl.replace(/\/+$/, '');
    this.indexerUrl = indexerUrl.replace(/\/+$/, '');
    this.fetchFn = fetchFn;
  }

  private async get<T>(url: string): Promise<T> {
    let res: Response;
    try {
      res = await this.fetchFn(url, { headers: { accept: 'application/json' } });
    } catch (e) {
      throw new ApiError(0, 'NETWORK', e instanceof Error ? e.message : 'network error');
    }
    if (!res.ok) {
      let code = `HTTP_${res.status}`;
      let message = res.statusText || 'request failed';
      try {
        const body = (await res.json()) as { error?: { code?: string; message?: string } | string };
        if (typeof body?.error === 'string') message = body.error;
        else if (body?.error) {
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

  client(addr: string) {
    return this.get<ClientRecord>(`${this.rpcUrl}/v1/clients/${encodeURIComponent(addr)}`);
  }
  account(addr: string) {
    return this.get<AccountRecord>(`${this.rpcUrl}/v1/accounts/${encodeURIComponent(addr)}`);
  }
  book(pair: string) {
    return this.get<Book>(`${this.rpcUrl}/v1/markets/${encodeURIComponent(pair)}/book?depth=10`);
  }
  custody(addr: string) {
    return this.get<Custody>(`${this.rpcUrl}/v1/custody/${encodeURIComponent(addr)}`);
  }
  async unlockDelaySecs() {
    const p = await this.get<{ budget?: { unlock_delay_secs?: number | string } }>(`${this.rpcUrl}/v1/params`);
    const v = Number(p.budget?.unlock_delay_secs);
    return Number.isFinite(v) ? v : null;
  }
  history(addr: string, cursor?: string) {
    const q = new URLSearchParams({ limit: '50' });
    if (cursor) q.set('cursor', cursor);
    return this.get<HistoryPage>(`${this.indexerUrl}/v1/accounts/${encodeURIComponent(addr)}/txs?${q.toString()}`);
  }
  assets() {
    return this.get<{ asset: string; decimals: number }[]>(`${this.indexerUrl}/v1/assets`);
  }
}
