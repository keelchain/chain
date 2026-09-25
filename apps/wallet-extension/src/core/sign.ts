/** Signing primitives: message signatures (§1) and action signatures, both via `@keelchain/sdk`. */
import * as ed from '@noble/ed25519';
import { sha512 } from '@noble/hashes/sha512';
import {
  messageDigest as sdkMessageDigest,
  signMessage as sdkSignMessage,
  verifyMessage as sdkVerifyMessage,
  txId,
  verify,
  type Action as SdkAction,
  type Keypair,
  type SignedAction as SdkSignedAction,
} from '@keelchain/sdk';
import type { Action, SignActionResult, SignedAction } from '../inpage/types';
import { WalletError } from './protocol';

// The SDK sets this on its own copy of @noble/ed25519; set it here too in
// case the bundler keeps two copies. Idempotent.
if (!ed.etc.sha512Sync) ed.etc.sha512Sync = (...m: Uint8Array[]) => sha512(ed.etc.concatBytes(...m));

/** sha256("keel-message-v1" ‖ utf8(message)) */
export function messageDigest(message: string): Uint8Array {
  return sdkMessageDigest(message);
}

export function signMessage(key: Keypair, message: string): string {
  return sdkSignMessage(key.secret, message);
}

export function verifyMessage(address: string, message: string, signatureHex: string): boolean {
  return sdkVerifyMessage(address, message, signatureHex);
}

export function signAction(key: Keypair, nonce: number, chainId: number, action: Action): SignActionResult {
  let signed: SdkSignedAction;
  try {
    signed = key.sign(nonce, chainId, action as unknown as SdkAction);
  } catch (e) {
    throw new WalletError('INVALID_REQUEST', `Cannot sign action: ${e instanceof Error ? e.message : String(e)}`);
  }
  return { signature: signed.signature, tx_id: txId(signed), signed: signed as unknown as SignedAction };
}

export function verifySignedAction(signed: SignedAction): boolean {
  try {
    return verify(signed as unknown as SdkSignedAction);
  } catch {
    return false;
  }
}
