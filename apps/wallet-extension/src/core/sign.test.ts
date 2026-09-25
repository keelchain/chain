import { describe, expect, it } from 'vitest';
import * as ed from '@noble/ed25519';
import { sha256 } from '@noble/hashes/sha256';
import { bytesToHex, hexToBytes, Keypair, txId, verify, verifyMessage as sdkVerifyMessage, type SignedAction } from '@keelchain/sdk';
import { messageDigest, signAction, signMessage, verifyMessage, verifySignedAction } from './sign';
import { accountKeypair, mnemonicToSeed } from './keys';
import { VECTOR } from './keys.test';

const key = accountKeypair(mnemonicToSeed(VECTOR.phrase), 0);

describe('message signing', () => {
  it('digest is sha256("keel-message-v1" || utf8(message))', () => {
    const msg = 'Keel login\naddress: abc\nnonce: 1';
    const expected = sha256(new Uint8Array([...new TextEncoder().encode('keel-message-v1'), ...new TextEncoder().encode(msg)]));
    expect(bytesToHex(messageDigest(msg))).toBe(bytesToHex(expected));
  });

  it('produces an Ed25519 signature over the digest that the SDK verifies', () => {
    const msg = 'Keel login\naddress: ' + key.address + '\nnonce: 42\nissued: 2026-09-09T00:00:00Z';
    const sig = signMessage(key, msg);
    expect(sig).toMatch(/^[0-9a-f]{128}$/);
    expect(verifyMessage(key.address, msg, sig)).toBe(true);
    expect(sdkVerifyMessage(key.address, msg, sig)).toBe(true);
    expect(ed.verify(hexToBytes(sig), messageDigest(msg), key.publicKey)).toBe(true);
    expect(verifyMessage(key.address, msg + ' ', sig)).toBe(false);
    expect(verifyMessage(Keypair.fromSeed(7).address, msg, sig)).toBe(false);
  });
});

describe('action signing', () => {
  it('signature verifies with the SDK and tx_id matches txId(signed)', () => {
    const r = signAction(key, 5, 1, { Transfer: { to: Keypair.fromSeed(2).address, asset: 'KEEL', amount: 1_500_000, memo: null } });
    expect(verify(r.signed as unknown as SignedAction)).toBe(true);
    expect(verifySignedAction(r.signed)).toBe(true);
    expect(r.tx_id).toBe(txId(r.signed as unknown as SignedAction));
    expect(r.signature).toBe(r.signed.signature);
    expect(r.signed.envelope.signer).toEqual(Array.from(key.publicKey));
    expect(r.signed.envelope.nonce).toBe(5);
    expect(r.signed.envelope.chain_id).toBe(1);
    // A different nonce changes the signature and tx id.
    const r2 = signAction(key, 6, 1, { Transfer: { to: Keypair.fromSeed(2).address, asset: 'KEEL', amount: 1_500_000, memo: null } });
    expect(r2.signature).not.toBe(r.signature);
    expect(r2.tx_id).not.toBe(r.tx_id);
  });

  it('signs AuthorizeSessionKey via the SDK encoder', () => {
    const r = signAction(key, 0, 1, { AuthorizeSessionKey: { key: Keypair.fromSeed(9).address, scope: 1, expires_at: 1_800_000_000 } });
    expect(verify(r.signed as unknown as SignedAction)).toBe(true);
    expect(r.tx_id).toBe(txId(r.signed as unknown as SignedAction));
  });
});
