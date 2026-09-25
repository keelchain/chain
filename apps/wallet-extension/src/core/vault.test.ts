import { describe, expect, it } from 'vitest';
import { decryptVault, encryptVault, isEncryptedVault, PBKDF2_ITERATIONS, WrongPasswordError } from './vault';

const MNEMONIC = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art';

describe('vault', () => {
  it('round-trips secrets through PBKDF2-SHA256 + AES-GCM', async () => {
    const v = await encryptVault({ mnemonic: MNEMONIC }, 'correct horse battery');
    expect(isEncryptedVault(v)).toBe(true);
    expect(v.kdf.iterations).toBe(PBKDF2_ITERATIONS);
    expect(v.kdf.iterations).toBeGreaterThanOrEqual(300_000);
    expect(v.kdf.salt).toHaveLength(32);
    expect(v.cipher.iv).toHaveLength(24);
    expect(v.ciphertext).not.toContain('abandon');
    const out = await decryptVault(v, 'correct horse battery');
    expect(out.mnemonic).toBe(MNEMONIC);
  });

  it('uses a fresh random salt and iv per encryption', async () => {
    const a = await encryptVault({ mnemonic: MNEMONIC }, 'password1', 300_000);
    const b = await encryptVault({ mnemonic: MNEMONIC }, 'password1', 300_000);
    expect(a.kdf.salt).not.toBe(b.kdf.salt);
    expect(a.cipher.iv).not.toBe(b.cipher.iv);
    expect(a.ciphertext).not.toBe(b.ciphertext);
  });

  it('rejects a wrong password', async () => {
    const v = await encryptVault({ mnemonic: MNEMONIC }, 'correct horse battery', 300_000);
    await expect(decryptVault(v, 'correct horse batter')).rejects.toBeInstanceOf(WrongPasswordError);
  });

  it('rejects tampered ciphertext', async () => {
    const v = await encryptVault({ mnemonic: MNEMONIC }, 'correct horse battery', 300_000);
    const flipped = (parseInt(v.ciphertext.slice(0, 2), 16) ^ 1).toString(16).padStart(2, '0') + v.ciphertext.slice(2);
    await expect(decryptVault({ ...v, ciphertext: flipped }, 'correct horse battery')).rejects.toBeInstanceOf(WrongPasswordError);
  });

  it('refuses short passwords and weak iteration counts', async () => {
    await expect(encryptVault({ mnemonic: MNEMONIC }, 'short')).rejects.toThrow(/at least 8/);
    await expect(encryptVault({ mnemonic: MNEMONIC }, 'long enough', 1000)).rejects.toThrow(/300,000/);
  });
});
