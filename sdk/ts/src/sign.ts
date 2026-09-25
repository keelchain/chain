// Envelope encoding, digest and ed25519 signing, byte-compatible with
// crates/keel-actions/src/envelope.rs.
import * as ed from "@noble/ed25519";
import { sha256 } from "@noble/hashes/sha256";
import { sha512 } from "@noble/hashes/sha512";
import { Writer, bytesToHex, hexToBytes } from "./borsh.ts";
import { encodeAction, toBytes, type Action, type Address } from "./actions.ts";

// @noble/ed25519 v2 needs a sync sha512 for the sync API.
(ed.etc as any).sha512Sync = (...m: Uint8Array[]) => sha512(ed.etc.concatBytes(...m));

export const CHAIN_ID_DEVNET = 1;
const DOMAIN = new TextEncoder().encode("keel-action-v1");
const TXID_DOMAIN = new TextEncoder().encode("keel-txid");
const MESSAGE_DOMAIN = new TextEncoder().encode("keel-message-v1");

/** Digest of an arbitrary message (login challenges): sha256("keel-message-v1" || utf8(message)). */
export function messageDigest(message: string): Uint8Array {
  return sha256(concat(MESSAGE_DOMAIN, new TextEncoder().encode(message)));
}

/** Ed25519 over `messageDigest`; hex. Never confusable with an action envelope (different domain tag). */
export function signMessage(secret: Uint8Array, message: string): string {
  return bytesToHex(ed.sign(messageDigest(message), secret));
}

export function verifyMessage(address: Address, message: string, signatureHex: string): boolean {
  try {
    return ed.verify(hexToBytes(signatureHex), messageDigest(message), toBytes(address));
  } catch {
    return false;
  }
}

export interface Envelope { signer: Address; nonce: bigint | number; chain_id: number; action: Action }

export interface SignedAction {
  envelope: { signer: number[]; nonce: number; chain_id: number; action: Action };
  signature: string;
}

export function encodeEnvelope(e: Envelope): Uint8Array {
  const w = new Writer();
  w.fixed(toBytes(e.signer), 32);
  w.u64(e.nonce);
  w.u32(e.chain_id);
  encodeAction(e.action, w);
  return w.bytes();
}

export function envelopeDigest(e: Envelope): Uint8Array {
  return sha256(concat(DOMAIN, encodeEnvelope(e)));
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}

export class Keypair {
  readonly secret: Uint8Array;
  readonly publicKey: Uint8Array;

  private constructor(secret: Uint8Array, publicKey: Uint8Array) {
    this.secret = secret;
    this.publicKey = publicKey;
  }

  static fromSecret(secret: Uint8Array | string): Keypair {
    const s = toBytes(secret);
    if (s.length !== 32) throw new Error("secret must be 32 bytes");
    return new Keypair(s, ed.getPublicKey(s));
  }

  /** Same derivation as `keel_crypto::Keypair::from_seed` (devnet accounts). */
  static fromSeed(seed: bigint | number): Keypair {
    const w = new Writer();
    w.u64(seed);
    const be = w.bytes().reverse(); // to_be_bytes
    return Keypair.fromSecret(sha256(concat(new TextEncoder().encode("keel-keypair-seed"), be)));
  }

  static random(): Keypair {
    return Keypair.fromSecret(ed.utils.randomPrivateKey());
  }

  get address(): string {
    return bytesToHex(this.publicKey);
  }

  sign(nonce: bigint | number, chainId: number, action: Action): SignedAction {
    const envelope: Envelope = { signer: this.publicKey, nonce, chain_id: chainId, action };
    const sig = ed.sign(envelopeDigest(envelope), this.secret);
    return {
      envelope: { signer: Array.from(this.publicKey), nonce: Number(nonce), chain_id: chainId, action },
      signature: bytesToHex(sig),
    };
  }
}

export function verify(sa: SignedAction): boolean {
  const e: Envelope = { signer: sa.envelope.signer, nonce: sa.envelope.nonce, chain_id: sa.envelope.chain_id, action: sa.envelope.action };
  return ed.verify(hexToBytes(sa.signature), envelopeDigest(e), toBytes(sa.envelope.signer));
}

export function txId(sa: SignedAction): string {
  const e: Envelope = { signer: sa.envelope.signer, nonce: sa.envelope.nonce, chain_id: sa.envelope.chain_id, action: sa.envelope.action };
  return bytesToHex(sha256(concat(TXID_DOMAIN, envelopeDigest(e), hexToBytes(sa.signature))));
}

/** JSON with bigints emitted as bare integers (serde u128 reads them). */
export function stringifyJson(v: unknown): string {
  return JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? `__BIG__${x.toString()}__` : x)).replace(/"__BIG__(-?\d+)__"/g, "$1");
}
