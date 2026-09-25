import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { encodeAction, encodeEnvelope, envelopeDigest, Keypair, bytesToHex, txId, verify } from "../src/index.ts";

const doc = JSON.parse(readFileSync(new URL("./vectors.json", import.meta.url), "utf8"));
const key = Keypair.fromSecret(doc.secret);

test("keypair matches the Rust derivation", () => {
  assert.equal(key.address, doc.address);
  assert.equal(Keypair.fromSeed(1).address, doc.address);
});

for (const v of doc.vectors) {
  test(`borsh + signature vector: ${v.name}`, () => {
    const action = v.action_json;
    assert.equal(bytesToHex(encodeAction(action).bytes()), v.action_borsh, "action borsh");
    const env = { signer: key.publicKey, nonce: BigInt(v.nonce), chain_id: v.chain_id, action };
    assert.equal(bytesToHex(encodeEnvelope(env)), v.envelope_borsh, "envelope borsh");
    assert.equal(bytesToHex(envelopeDigest(env)), v.digest, "digest");
    const sa = key.sign(BigInt(v.nonce), v.chain_id, action);
    assert.equal(sa.signature, v.signature, "signature (ed25519 is deterministic)");
    assert.equal(txId(sa), v.tx_id, "tx id");
    assert.ok(verify(sa));
  });
}
