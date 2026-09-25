/**
 * Key derivation (see README "Key derivation"):
 *
 *   seed      = BIP39-seed(mnemonic, passphrase = "")            (64 bytes)
 *   secret_0  = seed[0..32]                                     (account 0, matches docs/wallet.md)
 *   secret_i  = sha256(seed ‖ u32_le(i))          for i ≥ 1     (32 bytes)
 *   address_i = ed25519_public_key(secret_i)                     (hex, 32 bytes)
 *
 * The Ed25519 secret is the 32-byte seed as in RFC 8032 (the SDK's `Keypair.fromSecret`).
 */
import * as bip39 from '@scure/bip39';
import { wordlist } from '@scure/bip39/wordlists/english';
import { sha256 } from '@noble/hashes/sha256';
import { Keypair } from '@keelchain/sdk';

export const MNEMONIC_WORDS = 24;

export function generateMnemonic(): string {
  return bip39.generateMnemonic(wordlist, 256);
}

export function normalizeMnemonic(phrase: string): string {
  return phrase.trim().toLowerCase().split(/\s+/).join(' ');
}

export function validateMnemonic(phrase: string): boolean {
  const m = normalizeMnemonic(phrase);
  return m.split(' ').length === MNEMONIC_WORDS && bip39.validateMnemonic(m, wordlist);
}

export function mnemonicToSeed(phrase: string): Uint8Array {
  const m = normalizeMnemonic(phrase);
  if (!bip39.validateMnemonic(m, wordlist)) throw new Error('Invalid mnemonic');
  return bip39.mnemonicToSeedSync(m, '');
}

export function accountSecret(seed: Uint8Array, index: number): Uint8Array {
  if (seed.length !== 64) throw new Error('seed must be 64 bytes');
  if (!Number.isInteger(index) || index < 0 || index > 0xffffffff) throw new Error('bad account index');
  if (index === 0) return seed.slice(0, 32);
  const buf = new Uint8Array(64 + 4);
  buf.set(seed, 0);
  new DataView(buf.buffer).setUint32(64, index, true);
  return sha256(buf);
}

export function accountKeypair(seed: Uint8Array, index: number): Keypair {
  return Keypair.fromSecret(accountSecret(seed, index));
}

export function accountAddress(seed: Uint8Array, index: number): string {
  return accountKeypair(seed, index).address;
}
