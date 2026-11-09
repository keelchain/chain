/**
 * Deposit progress: what a user sees between "I sent the coins" and "the
 * chain credited me".
 *
 * The chain only learns about a deposit once an observer has seen it at the
 * required confirmation depth, so the first minutes of a deposit are invisible
 * to every Keel endpoint. These helpers look the deposit address up on the
 * external chain's public API (mempool.space for Bitcoin, TronGrid for Tron),
 * merge what they find with the chain's own deposit rows, and name the stage
 * each incoming transaction is at. They run in a browser (the wallet popup,
 * the explorer) as well as in Node; both APIs answer cross-origin requests.
 *
 * Ethereum has no free public API that works without a key, so it is listed
 * with its explorer only and its incoming transactions are not looked up.
 */

export type ExternalChain = "BTC" | "ETH" | "TRON";

export interface ExternalNetwork {
  chain: ExternalChain;
  /** What to call it in a UI: "Bitcoin signet", "Tron Nile", "Bitcoin". */
  name: string;
  /** Public API base, or null when incoming transactions cannot be looked up. */
  api: string | null;
  /** Block explorer base for links. */
  explorer: string;
}

/** The external networks a Keel network bridges. Anything that is not
 *  `mainnet` is the testnet set (Bitcoin signet, Tron Nile, Ethereum Sepolia). */
export function externalNetworks(keelNetwork: string): Record<ExternalChain, ExternalNetwork> {
  if (keelNetwork === "mainnet") {
    return {
      BTC: { chain: "BTC", name: "Bitcoin", api: "https://mempool.space/api", explorer: "https://mempool.space" },
      ETH: { chain: "ETH", name: "Ethereum", api: null, explorer: "https://etherscan.io" },
      TRON: { chain: "TRON", name: "Tron", api: "https://api.trongrid.io", explorer: "https://tronscan.org/#" },
    };
  }
  return {
    BTC: { chain: "BTC", name: "Bitcoin signet", api: "https://mempool.space/signet/api", explorer: "https://mempool.space/signet" },
    ETH: { chain: "ETH", name: "Ethereum Sepolia", api: null, explorer: "https://sepolia.etherscan.io" },
    TRON: { chain: "TRON", name: "Tron Nile", api: "https://nile.trongrid.io", explorer: "https://nile.tronscan.org/#" },
  };
}

export function externalTxUrl(net: ExternalNetwork, txHash: string): string {
  const hash = net.chain === "ETH" ? (txHash.startsWith("0x") ? txHash : `0x${txHash}`) : txHash.replace(/^0x/, "");
  return net.chain === "TRON" ? `${net.explorer}/transaction/${hash}` : `${net.explorer}/tx/${hash}`;
}

export function externalAddressUrl(net: ExternalNetwork, address: string): string {
  return `${net.explorer}/address/${address}`;
}

/** An incoming transaction to a deposit address, as the external chain sees it. */
export interface IncomingTx {
  chain: ExternalChain;
  tx_hash: string;
  /** Keel asset name: BTC.BTC, TRON.USDT, TRON.TRX. */
  asset: string;
  /** In the asset's smallest units, as a decimal string. */
  amount: string;
  /** 0 while unconfirmed; null when the API did not say. */
  confirmations: number | null;
  /** Unix seconds when the chain saw it, when known. */
  time: number | null;
}

// ---------------------------------------------------------------- parsers (pure, tested)

interface MempoolTx {
  txid: string;
  status: { confirmed: boolean; block_height?: number; block_time?: number };
  vout: { scriptpubkey_address?: string; value: number }[];
}

/** mempool.space `/address/{addr}/txs` rows paying `address`, with confirmations against `tip`. */
export function parseMempoolTxs(json: unknown, address: string, tip: number | null): IncomingTx[] {
  if (!Array.isArray(json)) return [];
  const out: IncomingTx[] = [];
  for (const t of json as MempoolTx[]) {
    if (!t || typeof t.txid !== "string" || !Array.isArray(t.vout)) continue;
    const paid = t.vout.filter((o) => o.scriptpubkey_address === address).reduce((s, o) => s + Number(o.value || 0), 0);
    if (paid <= 0) continue;
    const confirmed = Boolean(t.status?.confirmed);
    const height = t.status?.block_height;
    const confirmations = !confirmed ? 0 : tip !== null && typeof height === "number" ? Math.max(1, tip - height + 1) : null;
    out.push({ chain: "BTC", tx_hash: t.txid, asset: "BTC.BTC", amount: String(paid), confirmations, time: t.status?.block_time ?? null });
  }
  return out;
}

interface Trc20Row {
  transaction_id: string;
  token_info?: { symbol?: string; address?: string; decimals?: number };
  block_timestamp?: number;
  to?: string;
  type?: string;
  value?: string;
}

/** TronGrid `/v1/accounts/{addr}/transactions/trc20?only_to=true` rows. Only USDT is a
 *  Keel asset; `usdtContract`, when given, pins the token contract so look-alike tokens
 *  named USDT are ignored. Confirmations come later from `withTronConfirmations`. */
export function parseTrc20(json: unknown, address: string, usdtContract?: string | null): IncomingTx[] {
  const data = (json as { data?: unknown })?.data;
  if (!Array.isArray(data)) return [];
  const out: IncomingTx[] = [];
  for (const r of data as Trc20Row[]) {
    if (!r || typeof r.transaction_id !== "string" || r.type !== "Transfer" || r.to !== address) continue;
    const symbol = (r.token_info?.symbol ?? "").toUpperCase();
    if (symbol !== "USDT") continue;
    if (usdtContract && r.token_info?.address && r.token_info.address !== usdtContract) continue;
    if (!/^\d+$/.test(r.value ?? "")) continue;
    out.push({
      chain: "TRON",
      tx_hash: r.transaction_id,
      asset: "TRON.USDT",
      amount: r.value!,
      confirmations: null,
      time: typeof r.block_timestamp === "number" ? Math.floor(r.block_timestamp / 1000) : null,
    });
  }
  return out;
}

interface TrxRow {
  txID: string;
  blockNumber?: number;
  block_timestamp?: number;
  raw_data?: { contract?: { type?: string; parameter?: { value?: { amount?: number; to_address?: string } } }[] };
}

/** TronGrid `/v1/accounts/{addr}/transactions?only_to=true` rows that are plain TRX transfers. */
export function parseTrxTransfers(json: unknown, now: number | null): IncomingTx[] {
  const data = (json as { data?: unknown })?.data;
  if (!Array.isArray(data)) return [];
  const out: IncomingTx[] = [];
  for (const r of data as TrxRow[]) {
    const c = r?.raw_data?.contract?.[0];
    if (!r || typeof r.txID !== "string" || c?.type !== "TransferContract") continue;
    const amount = c.parameter?.value?.amount;
    if (typeof amount !== "number" || amount <= 0) continue;
    const confirmations = now !== null && typeof r.blockNumber === "number" ? Math.max(1, now - r.blockNumber + 1) : null;
    out.push({
      chain: "TRON",
      tx_hash: r.txID,
      asset: "TRON.TRX",
      amount: String(amount),
      confirmations,
      time: typeof r.block_timestamp === "number" ? Math.floor(r.block_timestamp / 1000) : null,
    });
  }
  return out;
}

// ---------------------------------------------------------------- fetchers

async function getJson(fetchImpl: typeof fetch, url: string, init?: RequestInit): Promise<unknown> {
  const res = await fetchImpl(url, init);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.json();
}

/** Incoming transactions to `address` on the external chain, newest first.
 *  Resolves to `null` when the network has no public API (Ethereum). Throws on a
 *  network error so the caller can say "could not reach <network>" rather than
 *  "nothing incoming". */
export async function fetchIncoming(
  net: ExternalNetwork,
  address: string,
  opts: { fetch?: typeof fetch; usdtContract?: string | null } = {},
): Promise<IncomingTx[] | null> {
  const f = opts.fetch ?? fetch;
  if (!net.api) return null;
  if (net.chain === "BTC") {
    const [txs, tipRaw] = await Promise.all([
      getJson(f, `${net.api}/address/${address}/txs`),
      f(`${net.api}/blocks/tip/height`).then((r) => (r.ok ? r.text() : "")).catch(() => ""),
    ]);
    const tip = /^\d+$/.test(tipRaw.trim()) ? Number(tipRaw.trim()) : null;
    return parseMempoolTxs(txs, address, tip);
  }
  if (net.chain === "TRON") {
    const [trc20, trx, nowBlock] = await Promise.all([
      getJson(f, `${net.api}/v1/accounts/${address}/transactions/trc20?only_to=true&only_confirmed=false&limit=20`),
      getJson(f, `${net.api}/v1/accounts/${address}/transactions?only_to=true&only_confirmed=false&limit=20`).catch(() => null),
      getJson(f, `${net.api}/wallet/getnowblock`, { method: "POST" }).catch(() => null),
    ]);
    const now = (nowBlock as { block_header?: { raw_data?: { number?: number } } } | null)?.block_header?.raw_data?.number ?? null;
    const usdt = parseTrc20(trc20, address, opts.usdtContract);
    await withTronConfirmations(net, usdt, now, f);
    const list = [...usdt, ...parseTrxTransfers(trx, now)];
    list.sort((a, b) => (b.time ?? 0) - (a.time ?? 0));
    return list;
  }
  return null;
}

/** TronGrid's TRC-20 listing has no block number; ask for each transaction's
 *  info to count confirmations. Failures leave `confirmations` null. */
async function withTronConfirmations(net: ExternalNetwork, txs: IncomingTx[], now: number | null, f: typeof fetch): Promise<void> {
  if (now === null || !net.api) return;
  await Promise.all(
    txs.map(async (t) => {
      try {
        const info = (await getJson(f, `${net.api}/wallet/gettransactioninfobyid`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ value: t.tx_hash }),
        })) as { blockNumber?: number };
        if (typeof info.blockNumber === "number") t.confirmations = Math.max(1, now - info.blockNumber + 1);
        else t.confirmations = 0;
      } catch {
        /* unknown */
      }
    }),
  );
}

// ---------------------------------------------------------------- stages

/** A deposit row as the node (`/v1/vaults/deposits`) or the indexer
 *  (`/v1/vaults/{chain}/deposits`) returns it; only the fields used here. */
export interface ChainDepositRow {
  chain: string;
  tx_hash: string;
  asset: string;
  amount: string | number;
  status: string;
  votes?: number;
  external_height?: number | null;
  release_height?: number | null;
}

export type DepositStage =
  | "seen" // in the external chain's mempool, no confirmation yet
  | "confirming" // confirmed, below the required depth
  | "observing" // deep enough; waiting for the observers' quorum
  | "held" // credited but delayed as a large deposit
  | "credited"
  | "rejected";

export interface DepositProgress {
  chain: string;
  tx_hash: string;
  asset: string;
  amount: string;
  stage: DepositStage;
  /** Confirmations seen on the external chain, when known. */
  confirmations: number | null;
  /** Confirmations the chain requires before observers report it. */
  required: number;
  /** Observer votes recorded on chain, when the chain knows the deposit. */
  votes: number | null;
  time: number | null;
  release_height: number | null;
}

/** Merge what the external chain shows with what the Keel chain recorded, newest
 *  first. A transaction the chain already credited keeps the chain's word even if
 *  the external lookup has not caught up; one the chain does not know yet is staged
 *  by its confirmations against `required`. */
export function depositProgress(incoming: IncomingTx[] | null, chainRows: ChainDepositRow[], required: number): DepositProgress[] {
  const key = (chain: string, hash: string) => `${chain}:${hash.replace(/^0x/, "").toLowerCase()}`;
  const byKey = new Map<string, DepositProgress>();
  for (const r of chainRows) {
    const status = r.status.toLowerCase();
    const stage: DepositStage =
      status === "credited" ? "credited" : status === "held" ? "held" : status === "rejected" ? "rejected" : "observing";
    byKey.set(key(r.chain, r.tx_hash), {
      chain: r.chain,
      tx_hash: r.tx_hash,
      asset: r.asset,
      amount: String(r.amount),
      stage,
      confirmations: null,
      required,
      votes: typeof r.votes === "number" ? r.votes : null,
      time: null,
      release_height: r.release_height ?? null,
    });
  }
  for (const t of incoming ?? []) {
    const k = key(t.chain, t.tx_hash);
    const known = byKey.get(k);
    if (known) {
      known.confirmations = t.confirmations;
      known.time = t.time;
      continue;
    }
    const stage: DepositStage = t.confirmations === null ? "confirming" : t.confirmations === 0 ? "seen" : t.confirmations < required ? "confirming" : "observing";
    byKey.set(k, {
      chain: t.chain,
      tx_hash: t.tx_hash,
      asset: t.asset,
      amount: t.amount,
      stage,
      confirmations: t.confirmations,
      required,
      votes: null,
      time: t.time,
      release_height: null,
    });
  }
  const order: Record<DepositStage, number> = { seen: 0, confirming: 1, observing: 2, held: 3, rejected: 4, credited: 5 };
  return [...byKey.values()].sort((a, b) => order[a.stage] - order[b.stage] || (b.time ?? 0) - (a.time ?? 0));
}

/** True for stages a user is still waiting on. */
export function isProcessing(stage: DepositStage): boolean {
  return stage === "seen" || stage === "confirming" || stage === "observing" || stage === "held";
}

/** Plain words for a stage, for a badge or a sentence. */
export function stageLabel(p: DepositProgress): string {
  switch (p.stage) {
    case "seen":
      return "Seen, waiting for the first confirmation";
    case "confirming":
      return p.confirmations === null ? "Confirming" : `${p.confirmations} of ${p.required} confirmations`;
    case "observing":
      return p.votes ? `Observers voting (${p.votes})` : "Confirmed, waiting for the observers";
    case "held":
      return p.release_height ? `Credited, usable at block ${p.release_height}` : "Credited, delayed as a large deposit";
    case "credited":
      return "Credited";
    case "rejected":
      return "Rejected";
  }
}
