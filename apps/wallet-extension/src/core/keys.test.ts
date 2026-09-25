import { describe, expect, it } from 'vitest';
import { bytesToHex } from '@keelchain/sdk';
import { accountAddress, accountSecret, generateMnemonic, mnemonicToSeed, validateMnemonic } from './keys';

/**
 * Key derivation vector (also in README.md):
 *   phrase   = "abandon" ×23 + "art" (BIP39 English, empty passphrase)
 *   seed     = 408b285c…480840 (64 bytes)
 *   secret_0 = seed[0..32]              → address_0 below
 *   secret_1 = sha256(seed ‖ u32le(1))  → address_1 below
 */
export const VECTOR = {
  phrase: 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art',
  seed: '408b285c123836004f4b8842c89324c1f01382450c0d439af345ba7fc49acf705489c6fc77dbd4e3dc1dd8cc6bc9f043db8ada1e243c4a0eafb290d399480840',
  secret0: '408b285c123836004f4b8842c89324c1f01382450c0d439af345ba7fc49acf70',
  address0: '1de352e44cd333672593f2334a730e180aaf290de89aa16d480de594e34e2961',
  secret1: 'd08127fabcc24eb0b83b2f129c9c31c06109e211946301fac107f7b7125802e4',
  address1: '59424260f06f5414dec45bd8b9354bef75425bdc79c40f70d38e838e8831ff97',
};

describe('key derivation', () => {
  it('derives the documented seed and addresses from the fixed phrase', () => {
    expect(validateMnemonic(VECTOR.phrase)).toBe(true);
    const seed = mnemonicToSeed(VECTOR.phrase);
    expect(bytesToHex(seed)).toBe(VECTOR.seed);
    expect(bytesToHex(accountSecret(seed, 0))).toBe(VECTOR.secret0);
    expect(accountAddress(seed, 0)).toBe(VECTOR.address0);
    expect(bytesToHex(accountSecret(seed, 1))).toBe(VECTOR.secret1);
    expect(accountAddress(seed, 1)).toBe(VECTOR.address1);
  });

  it('is deterministic and case/whitespace tolerant', () => {
    const a = accountAddress(mnemonicToSeed(VECTOR.phrase), 0);
    const b = accountAddress(mnemonicToSeed(`  ${VECTOR.phrase.toUpperCase().replace(/ /g, '\n')} `), 0);
    expect(a).toBe(b);
  });

  it('account 0 is the first 32 bytes of the BIP39 seed', () => {
    const seed = mnemonicToSeed(VECTOR.phrase);
    expect(accountSecret(seed, 0)).toEqual(seed.slice(0, 32));
  });

  it('rejects invalid phrases and generates valid 24-word ones', () => {
    expect(validateMnemonic('abandon abandon abandon')).toBe(false);
    expect(validateMnemonic(VECTOR.phrase.replace(/art$/, 'zoo'))).toBe(false);
    const m = generateMnemonic();
    expect(m.split(' ')).toHaveLength(24);
    expect(validateMnemonic(m)).toBe(true);
    expect(generateMnemonic()).not.toBe(m);
  });
});
