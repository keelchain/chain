import { describe, expect, it } from 'vitest';
import { hex } from '@scure/base';
import { secp256k1 } from '@noble/curves/secp256k1.js';
import { sha256 } from '@noble/hashes/sha256';
import { mnemonicToSeed } from './keys';
import {
  btcAddressFromPubkey,
  buildBtcTransfer,
  hdPublicChild,
  isBtcAddress,
  selfKey,
  selfPath,
  tronAddressFromPubkey,
  tronCreateTrc20,
  tronDecode,
  tronSign,
  type TronTx,
} from './selfcustody';

// The BIP39 test phrase; BIP84's own vector for its first receive address.
const PHRASE = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art';

// The public testnet's vault key and the two live deposit addresses it derives
// for index 1 (chain paths m/0/1 and m/195/1): a cross-check of the Bitcoin
// bech32 and Tron base58check encodings against the Rust implementation.
const VAULT_PK = hex.decode('029b992a341b20cc5e03ffe3a2b744b057ff5c79b8dc79d28e60eba8a04f49734f');
const VAULT_CC = hex.decode('753f5c955c6c01ad6435939ef31fe55306ad1c3a73a3307adbb783002c2da6ed');

describe('self-custody keys', () => {
  it('derives the standard paths from the same seed as the Keel accounts', () => {
    const seed = mnemonicToSeed(PHRASE);
    expect(selfPath('BTC', 0, 'mainnet')).toBe("m/84'/0'/0'/0/0");
    expect(selfPath('BTC', 2, 'testnet')).toBe("m/84'/1'/0'/0/2");
    expect(selfPath('TRON', 0, 'testnet')).toBe("m/44'/195'/0'/0/0");
    const main = selfKey(seed, 'BTC', 0, 'mainnet');
    expect(main.address.startsWith('bc1q')).toBe(true);
    const test = selfKey(seed, 'BTC', 0, 'testnet');
    expect(test.address.startsWith('tb1q')).toBe(true);
    expect(test.address).not.toBe(main.address);
    const tron = selfKey(seed, 'TRON', 0, 'testnet');
    expect(tron.address.startsWith('T')).toBe(true);
    expect(tronDecode(tron.address)).not.toBeNull();
    expect(selfKey(seed, 'TRON', 0, 'mainnet').address).toBe(tron.address);
    expect(selfKey(seed, 'TRON', 1, 'mainnet').address).not.toBe(tron.address);
    expect(isBtcAddress(test.address, 'testnet')).toBe(true);
    expect(isBtcAddress(test.address, 'mainnet')).toBe(false);
  });

  it('encodes Bitcoin and Tron addresses exactly as the chain does for its vault children', () => {
    expect(btcAddressFromPubkey(hdPublicChild(VAULT_PK, VAULT_CC, [0, 1]), 'testnet')).toBe('tb1q3nyvdyv9f44m5zmrduumvneynpmut0fxm68gn8');
    expect(tronAddressFromPubkey(hdPublicChild(VAULT_PK, VAULT_CC, [195, 1]))).toBe('TYqumPzuwGPqBP4JiouDjr8ajD6g4ThPXY');
    const raw = tronDecode('TYqumPzuwGPqBP4JiouDjr8ajD6g4ThPXY')!;
    expect(raw[0]).toBe(0x41);
    expect(tronDecode('TYqumPzuwGPqBP4JiouDjr8ajD6g4ThPXZ')).toBeNull();
    expect(tronDecode('tb1q3nyvdyv9f44m5zmrduumvneynpmut0fxm68gn8')).toBeNull();
  });
});

describe('moving coins to Keel', () => {
  const seed = mnemonicToSeed(PHRASE);
  const key = selfKey(seed, 'BTC', 0, 'testnet');
  const utxos = [
    { txid: '11'.repeat(32), vout: 0, value: 50_000n, confirmed: true },
    { txid: '22'.repeat(32), vout: 1, value: 70_000n, confirmed: true },
  ];
  const vault = 'tb1q3nyvdyv9f44m5zmrduumvneynpmut0fxm68gn8';

  it('builds a signed Bitcoin spend with change back to the own address', () => {
    const built = buildBtcTransfer({ key, utxos, to: vault, amount: 60_000n, feeRate: 2, net: 'testnet' });
    expect(built.amount).toBe(60_000n);
    expect(built.fee).toBeGreaterThan(0n);
    expect(built.fee).toBeLessThan(2_000n);
    expect(built.hex.length).toBeGreaterThan(200);
    expect(built.txid).toMatch(/^[0-9a-f]{64}$/);
    expect(() => buildBtcTransfer({ key, utxos, to: vault, amount: 500_000n, feeRate: 2, net: 'testnet' })).toThrow(/Not enough/);
  });

  it('send-max spends every coin and takes the fee from the amount', () => {
    const built = buildBtcTransfer({ key, utxos, to: vault, amount: 0n, feeRate: 3, net: 'testnet', sendMax: true });
    expect(built.amount + built.fee).toBe(120_000n);
    expect(built.fee).toBe(3n * BigInt(11 + 68 * 2 + 31));
  });

  it('signs a Tron transaction as r ‖ s ‖ (27 + v) over sha256 of the raw bytes', () => {
    const tron = selfKey(seed, 'TRON', 0, 'testnet');
    const rawHex = '0a02' + 'ab'.repeat(60);
    const tx: TronTx = { txID: hex.encode(sha256(hex.decode(rawHex))), raw_data: {}, raw_data_hex: rawHex };
    const signed = tronSign(tx, tron.privateKey);
    const sig = hex.decode(signed.signature![0]!);
    expect(sig.length).toBe(65);
    expect(sig[64]).toBeGreaterThanOrEqual(27);
    expect(sig[64]).toBeLessThanOrEqual(28);
    const nobleOrder = new Uint8Array(65);
    nobleOrder[0] = sig[64]! - 27;
    nobleOrder.set(sig.subarray(0, 64), 1);
    const recovered = secp256k1.recoverPublicKey(nobleOrder, hex.decode(tx.txID), { prehash: false });
    expect(hex.encode(recovered)).toBe(hex.encode(tron.publicKey));
    expect(() => tronSign({ ...tx, txID: '00'.repeat(32) }, tron.privateKey)).toThrow(/does not match/);
  });

  it('encodes a TRC-20 transfer call for TronGrid', async () => {
    let body: Record<string, unknown> = {};
    const f = (async (_url: RequestInfo | URL, init?: RequestInit) => {
      body = JSON.parse(String(init?.body)) as Record<string, unknown>;
      return Response.json({ result: { result: true }, transaction: { txID: 'aa', raw_data: {}, raw_data_hex: 'aa' } });
    }) as unknown as typeof fetch;
    await tronCreateTrc20('https://nile.trongrid.io', 'TAAAA', 'TYqumPzuwGPqBP4JiouDjr8ajD6g4ThPXY', 1_000_000n, 'TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf', f);
    expect(body['function_selector']).toBe('transfer(address,uint256)');
    const param = String(body['parameter']);
    expect(param.length).toBe(128);
    expect(param.slice(0, 24)).toBe('0'.repeat(24));
    expect(param.slice(24, 64)).toBe(hex.encode(tronDecode('TYqumPzuwGPqBP4JiouDjr8ajD6g4ThPXY')!.subarray(1)));
    expect(BigInt('0x' + param.slice(64))).toBe(1_000_000n);
    expect(body['visible']).toBe(true);
  });
});
