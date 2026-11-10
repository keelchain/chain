/**
 * "In your wallet": coins the user holds under their own key on the external
 * chain, next to the "in Keel" balance the network vault holds for them.
 *
 * Keys come from the same BIP39 seed as the Keel accounts, on the standard
 * paths other wallets use, so the phrase alone recovers them anywhere:
 *
 *   Bitcoin  BIP84  m/84'/<coin>'/0'/0/<account>   coin 0 mainnet, 1 signet   bc1… / tb1…
 *   Tron     BIP44  m/44'/195'/0'/0/<account>                                  T…
 *
 * The chain never learns these addresses. Balances are read from the public
 * APIs the deposit tracker already uses; "move to Keel" builds, signs and
 * broadcasts a transaction from here to the account's vault deposit address;
 * "move to your wallet" is an ordinary `Withdraw` to one of these addresses.
 */
import { HDKey } from '@scure/bip32';
import { secp256k1 } from '@noble/curves/secp256k1.js';
import { keccak_256 } from '@noble/hashes/sha3';
import { sha256 } from '@noble/hashes/sha256';
import { base58, hex } from '@scure/base';
import * as btc from '@scure/btc-signer';

export type SelfChain = 'BTC' | 'TRON';
/** `testnet` is Bitcoin signet and Tron Nile; anything but `mainnet`. */
export type ExtNetwork = 'mainnet' | 'testnet';

export interface SelfKey {
  chain: SelfChain;
  path: string;
  privateKey: Uint8Array;
  /** Compressed secp256k1 key, 33 bytes. */
  publicKey: Uint8Array;
  address: string;
}

export function extNetwork(keelNetworkId: string): ExtNetwork {
  return keelNetworkId === 'mainnet' ? 'mainnet' : 'testnet';
}

export function selfPath(chain: SelfChain, account: number, net: ExtNetwork): string {
  if (!Number.isInteger(account) || account < 0 || account >= 0x8000_0000) throw new Error('bad account index');
  return chain === 'BTC' ? `m/84'/${net === 'mainnet' ? 0 : 1}'/0'/0/${account}` : `m/44'/195'/0'/0/${account}`;
}

/** The user's own key for `chain` and Keel account `account`. Wipe `privateKey` after use. */
export function selfKey(seed: Uint8Array, chain: SelfChain, account: number, net: ExtNetwork): SelfKey {
  if (seed.length !== 64) throw new Error('seed must be 64 bytes');
  const path = selfPath(chain, account, net);
  const node = HDKey.fromMasterSeed(seed).derive(path);
  if (!node.privateKey || !node.publicKey) throw new Error('derivation failed');
  const publicKey = node.publicKey;
  return { chain, path, privateKey: node.privateKey, publicKey, address: chain === 'BTC' ? btcAddressFromPubkey(publicKey, net) : tronAddressFromPubkey(publicKey) };
}

/** Non-hardened public derivation under a vault key, as the chain does for deposit addresses. */
export function hdPublicChild(pubkey33: Uint8Array, chainCode: Uint8Array, path: number[]): Uint8Array {
  let node = new HDKey({ publicKey: pubkey33, chainCode });
  for (const i of path) node = node.deriveChild(i);
  if (!node.publicKey) throw new Error('derivation failed');
  return node.publicKey;
}

export function btcNetwork(net: ExtNetwork): typeof btc.NETWORK {
  return net === 'mainnet' ? btc.NETWORK : btc.TEST_NETWORK;
}

export function btcAddressFromPubkey(pubkey33: Uint8Array, net: ExtNetwork): string {
  const a = btc.p2wpkh(pubkey33, btcNetwork(net)).address;
  if (!a) throw new Error('no address');
  return a;
}

function base58check(payload: Uint8Array): string {
  const check = sha256(sha256(payload)).slice(0, 4);
  const out = new Uint8Array(payload.length + 4);
  out.set(payload, 0);
  out.set(check, payload.length);
  return base58.encode(out);
}

/** Tron address: 0x41 ‖ last 20 bytes of keccak256(uncompressed key without its 0x04), base58check. */
export function tronAddressFromPubkey(pubkey33: Uint8Array): string {
  const uncompressed = secp256k1.Point.fromBytes(pubkey33).toBytes(false);
  const account = keccak_256(uncompressed.subarray(1)).slice(12);
  const raw = new Uint8Array(21);
  raw[0] = 0x41;
  raw.set(account, 1);
  return base58check(raw);
}

/** The 21 raw bytes (0x41 ‖ account) of a Tron address, or null when it is not one. */
export function tronDecode(address: string): Uint8Array | null {
  let bytes: Uint8Array;
  try {
    bytes = base58.decode(address);
  } catch {
    return null;
  }
  if (bytes.length !== 25 || bytes[0] !== 0x41) return null;
  const payload = bytes.subarray(0, 21);
  const check = sha256(sha256(payload)).slice(0, 4);
  for (let i = 0; i < 4; i++) if (check[i] !== bytes[21 + i]) return null;
  return payload;
}

export function isBtcAddress(address: string, net: ExtNetwork): boolean {
  try {
    btc.Address(btcNetwork(net)).decode(address);
    return true;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------- public APIs

export interface ExternalApis {
  /** mempool.space API base, e.g. https://mempool.space/signet/api */
  btc: string;
  /** TronGrid base, e.g. https://nile.trongrid.io */
  tron: string;
  /** The USDT TRC-20 contract on that Tron network. */
  usdt: string;
}

export function externalApis(net: ExtNetwork, usdtContract?: string): ExternalApis {
  return net === 'mainnet'
    ? { btc: 'https://mempool.space/api', tron: 'https://api.trongrid.io', usdt: usdtContract ?? 'TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t' }
    : { btc: 'https://mempool.space/signet/api', tron: 'https://nile.trongrid.io', usdt: usdtContract ?? 'TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf' };
}

async function getJson(f: typeof fetch, url: string, init?: RequestInit): Promise<unknown> {
  const res = await f(url, init);
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return res.json();
}

export interface BtcUtxo {
  txid: string;
  vout: number;
  value: bigint;
  confirmed: boolean;
}

export async function btcUtxos(api: string, address: string, f: typeof fetch = fetch): Promise<BtcUtxo[]> {
  const rows = (await getJson(f, `${api}/address/${address}/utxo`)) as { txid: string; vout: number; value: number; status?: { confirmed?: boolean } }[];
  if (!Array.isArray(rows)) return [];
  return rows.map((r) => ({ txid: r.txid, vout: r.vout, value: BigInt(r.value), confirmed: Boolean(r.status?.confirmed) }));
}

/** sat/vB for a confirmation within about half an hour; falls back to 2 when the API is down. */
export async function btcFeeRate(api: string, f: typeof fetch = fetch): Promise<number> {
  try {
    const r = (await getJson(f, `${api}/v1/fees/recommended`)) as { halfHourFee?: number; fastestFee?: number };
    const v = Number(r.halfHourFee ?? r.fastestFee ?? 0);
    return v >= 1 ? Math.ceil(v) : 2;
  } catch {
    return 2;
  }
}

export interface BuiltBtcTx {
  hex: string;
  txid: string;
  fee: bigint;
  vsize: number;
  /** What the destination receives (the whole balance minus the fee when `sendMax`). */
  amount: bigint;
}

/**
 * One P2WPKH spend from the user's key to `to`, change back to the same address.
 * With `sendMax` every coin is spent and the fee comes out of the amount.
 */
export function buildBtcTransfer(args: { key: SelfKey; utxos: BtcUtxo[]; to: string; amount: bigint; feeRate: number; net: ExtNetwork; sendMax?: boolean }): BuiltBtcTx {
  const { key, to, net } = args;
  const network = btcNetwork(net);
  const spend = btc.p2wpkh(key.publicKey, network);
  const inputs = args.utxos.map((u) => ({ txid: hex.decode(u.txid), index: u.vout, witnessUtxo: { script: spend.script, amount: u.value } }));
  if (!inputs.length) throw new Error('No coins to spend.');
  const feePerByte = BigInt(Math.max(1, Math.ceil(args.feeRate)));
  let outputs: { address: string; amount: bigint }[];
  let strategy: 'all' | 'default' = 'default';
  if (args.sendMax) {
    // Estimate the fee for an all-inputs, one-output transaction, then pay the rest.
    const total = args.utxos.reduce((s, u) => s + u.value, 0n);
    const vsize = 11 + 68 * inputs.length + 31;
    const fee = BigInt(vsize) * feePerByte;
    if (total <= fee) throw new Error('The balance does not cover the network fee.');
    outputs = [{ address: to, amount: total - fee }];
    strategy = 'all';
  } else {
    if (args.amount <= 0n) throw new Error('Enter an amount.');
    outputs = [{ address: to, amount: args.amount }];
  }
  const sel = btc.selectUTXO(inputs, outputs, strategy, { feePerByte, changeAddress: key.address, network, createTx: true, bip69: false, alwaysChange: false });
  if (!sel || !sel.tx) throw new Error('Not enough coins for that amount and the network fee.');
  const tx = sel.tx;
  tx.sign(key.privateKey);
  tx.finalize();
  const paid = outputs[0]!.amount;
  return { hex: tx.hex, txid: tx.id, fee: tx.fee, vsize: tx.vsize, amount: paid };
}

export async function broadcastBtc(api: string, rawHex: string, f: typeof fetch = fetch): Promise<string> {
  const res = await f(`${api}/tx`, { method: 'POST', body: rawHex });
  const text = await res.text();
  if (!res.ok) throw new Error(`Broadcast refused: ${text.slice(0, 200)}`);
  return text.trim();
}

// ---------------------------------------------------------------- Tron

export interface TronBalances {
  /** TRX in sun. */
  trx: bigint;
  /** USDT in its 6-decimal units. */
  usdt: bigint;
}

export async function tronBalances(api: string, address: string, usdtContract: string, f: typeof fetch = fetch): Promise<TronBalances> {
  const [acctJson, usdtFromContract] = await Promise.all([
    getJson(f, `${api}/v1/accounts/${address}`).catch(() => null) as Promise<{ data?: { balance?: number; trc20?: Record<string, string>[] }[] } | null>,
    trc20BalanceOf(api, usdtContract, address, f).catch(() => null),
  ]);
  const acct = acctJson?.data?.[0];
  // An address that only ever received tokens has no account record on Tron,
  // so `/v1/accounts` answers with nothing; the token contract still knows.
  let usdt = usdtFromContract;
  if (usdt === null) {
    usdt = 0n;
    for (const entry of acct?.trc20 ?? []) {
      const v = entry[usdtContract];
      if (v && /^\d+$/.test(v)) usdt += BigInt(v);
    }
  }
  return { trx: BigInt(acct?.balance ?? 0), usdt };
}

/** `balanceOf(address)` on a TRC-20 contract through `triggerconstantcontract`. */
export async function trc20BalanceOf(api: string, contract: string, address: string, f: typeof fetch = fetch): Promise<bigint> {
  const raw = tronDecode(address);
  if (!raw) throw new Error('Not a Tron address.');
  const j = (await getJson(f, `${api}/wallet/triggerconstantcontract`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ owner_address: address, contract_address: contract, function_selector: 'balanceOf(address)', parameter: hex.encode(raw.subarray(1)).padStart(64, '0'), visible: true }),
  })) as { result?: { result?: boolean }; constant_result?: string[] };
  const word = j.constant_result?.[0];
  if (!j.result?.result || !word || !/^[0-9a-fA-F]+$/.test(word)) throw new Error('balanceOf failed');
  return BigInt('0x' + word);
}

/** An unsigned Tron transaction as TronGrid returns it. */
export interface TronTx {
  txID: string;
  raw_data: unknown;
  raw_data_hex: string;
  visible?: boolean;
  signature?: string[];
}

function tronError(j: unknown): string | null {
  const o = j as { Error?: string; result?: { message?: string; result?: boolean; code?: string } };
  if (typeof o.Error === 'string') return o.Error;
  if (o.result && o.result.result === false) return o.result.message ? hexToAscii(o.result.message) : (o.result.code ?? 'refused');
  return null;
}

function hexToAscii(h: string): string {
  try {
    return new TextDecoder().decode(hex.decode(h));
  } catch {
    return h;
  }
}

/** Unsigned TRX transfer, amounts in sun. */
export async function tronCreateTrx(api: string, from: string, to: string, amountSun: bigint, f: typeof fetch = fetch): Promise<TronTx> {
  const j = await getJson(f, `${api}/wallet/createtransaction`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ owner_address: from, to_address: to, amount: Number(amountSun), visible: true }),
  });
  const err = tronError(j);
  if (err) throw new Error(`Tron: ${err}`);
  return j as TronTx;
}

/** Default fee limit for a TRC-20 transfer: 30 TRX, which covers the energy of a transfer to a fresh address. */
export const TRC20_FEE_LIMIT_SUN = 30_000_000;

/** Unsigned TRC-20 `transfer(to, amount)` call on `contract`. */
export async function tronCreateTrc20(api: string, from: string, to: string, amount: bigint, contract: string, f: typeof fetch = fetch, feeLimit = TRC20_FEE_LIMIT_SUN): Promise<TronTx> {
  const raw = tronDecode(to);
  if (!raw) throw new Error('Not a Tron address.');
  const parameter = hex.encode(raw.subarray(1)).padStart(64, '0') + amount.toString(16).padStart(64, '0');
  const j = (await getJson(f, `${api}/wallet/triggersmartcontract`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ owner_address: from, contract_address: contract, function_selector: 'transfer(address,uint256)', parameter, fee_limit: feeLimit, call_value: 0, visible: true }),
  })) as { transaction?: TronTx; result?: { result?: boolean; message?: string } };
  const err = tronError(j);
  if (err) throw new Error(`Tron: ${err}`);
  if (!j.transaction) throw new Error('Tron: no transaction returned');
  return j.transaction;
}

/** Signs with the user's key: sha256 of the raw transaction, secp256k1, r ‖ s ‖ v
 *  with v = 27 + recovery id, the header byte java-tron accepts (27..34). */
export function tronSign(tx: TronTx, privateKey: Uint8Array): TronTx {
  const digest = sha256(hex.decode(tx.raw_data_hex));
  if (hex.encode(digest) !== tx.txID) throw new Error('Tron: transaction id does not match its contents');
  const recovered = secp256k1.sign(digest, privateKey, { prehash: false, format: 'recovered' });
  // noble puts the recovery byte first; Tron wants it last, offset by 27.
  const sig = new Uint8Array(65);
  sig.set(recovered.subarray(1), 0);
  sig[64] = 27 + recovered[0]!;
  return { ...tx, signature: [hex.encode(sig)] };
}

export async function tronBroadcast(api: string, tx: TronTx, f: typeof fetch = fetch): Promise<string> {
  const j = (await getJson(f, `${api}/wallet/broadcasttransaction`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(tx) })) as {
    result?: boolean;
    txid?: string;
    code?: string;
    message?: string;
  };
  if (!j.result) throw new Error(`Tron broadcast refused: ${j.code ?? ''} ${j.message ? hexToAscii(j.message) : ''}`.trim());
  return j.txid ?? tx.txID;
}
