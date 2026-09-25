/**
 * The encrypted vault: password → PBKDF2-SHA256 (random 16-byte salt,
 * 310,000 iterations) → 256-bit AES-GCM key → JSON blob. Only WebCrypto is
 * used so the same code runs in the service worker, the popup and tests.
 */
import { bytesToHex, hexToBytes } from '@keelchain/sdk';

export const PBKDF2_ITERATIONS = 310_000;
export const VAULT_VERSION = 1;

export interface VaultSecrets {
  /** BIP39 English mnemonic, 24 words, single-space separated. */
  mnemonic: string;
}

export interface EncryptedVault {
  version: typeof VAULT_VERSION;
  kdf: { name: 'PBKDF2-SHA256'; iterations: number; salt: string };
  cipher: { name: 'AES-GCM'; iv: string };
  ciphertext: string;
}

export class WrongPasswordError extends Error {
  constructor() {
    super('Wrong password');
    this.name = 'WrongPasswordError';
  }
}

const subtle = (): SubtleCrypto => {
  const c = globalThis.crypto;
  if (!c?.subtle) throw new Error('WebCrypto is not available');
  return c.subtle;
};

async function deriveKey(password: string, salt: Uint8Array, iterations: number, usage: KeyUsage[]): Promise<CryptoKey> {
  const s = subtle();
  const material = await s.importKey('raw', new TextEncoder().encode(password.normalize('NFKC')), 'PBKDF2', false, ['deriveKey']);
  return s.deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt: salt as BufferSource, iterations }, material, { name: 'AES-GCM', length: 256 }, false, usage);
}

export async function encryptVault(secrets: VaultSecrets, password: string, iterations = PBKDF2_ITERATIONS): Promise<EncryptedVault> {
  if (password.length < 8) throw new Error('Password must be at least 8 characters');
  if (iterations < 300_000) throw new Error('PBKDF2 iterations must be at least 300,000');
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveKey(password, salt, iterations, ['encrypt']);
  const plaintext = new TextEncoder().encode(JSON.stringify(secrets));
  const ct = await subtle().encrypt({ name: 'AES-GCM', iv: iv as BufferSource }, key, plaintext as BufferSource);
  return {
    version: VAULT_VERSION,
    kdf: { name: 'PBKDF2-SHA256', iterations, salt: bytesToHex(salt) },
    cipher: { name: 'AES-GCM', iv: bytesToHex(iv) },
    ciphertext: bytesToHex(new Uint8Array(ct)),
  };
}

export async function decryptVault(vault: EncryptedVault, password: string): Promise<VaultSecrets> {
  if (vault.version !== VAULT_VERSION || vault.kdf.name !== 'PBKDF2-SHA256' || vault.cipher.name !== 'AES-GCM') throw new Error('Unsupported vault format');
  const key = await deriveKey(password, hexToBytes(vault.kdf.salt), vault.kdf.iterations, ['decrypt']);
  let pt: ArrayBuffer;
  try {
    pt = await subtle().decrypt({ name: 'AES-GCM', iv: hexToBytes(vault.cipher.iv) as BufferSource }, key, hexToBytes(vault.ciphertext) as BufferSource);
  } catch {
    throw new WrongPasswordError();
  }
  const parsed: unknown = JSON.parse(new TextDecoder().decode(pt));
  if (typeof parsed !== 'object' || parsed === null || typeof (parsed as VaultSecrets).mnemonic !== 'string') throw new Error('Corrupt vault');
  return { mnemonic: (parsed as VaultSecrets).mnemonic };
}

export function isEncryptedVault(v: unknown): v is EncryptedVault {
  if (typeof v !== 'object' || v === null) return false;
  const r = v as Record<string, unknown>;
  return r['version'] === VAULT_VERSION && typeof r['ciphertext'] === 'string' && typeof r['kdf'] === 'object' && typeof r['cipher'] === 'object';
}
