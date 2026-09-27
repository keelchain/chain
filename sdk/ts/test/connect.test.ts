// Login challenges: issued, signed as the wallet would, verified server side.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Keypair } from "../src/sign.ts";
import { loginChallenge, parseLoginChallenge, signLoginWith, verifyLoginChallenge } from "../src/connect.ts";

test("a challenge round-trips through parse and verify", () => {
  const key = Keypair.fromSeed(5n);
  const issued = new Date("2026-09-27T12:00:00Z");
  const msg = loginChallenge(key.address, { nonce: "abc123", issued, site: "exchange.example" });
  assert.equal(msg.split("\n")[0], "Keel login");
  const parsed = parseLoginChallenge(msg)!;
  assert.equal(parsed.address, key.address);
  assert.equal(parsed.nonce, "abc123");
  assert.equal(parsed.site, "exchange.example");
  const { signature } = signLoginWith(key, msg);
  const ok = verifyLoginChallenge(key.address, msg, signature, { now: new Date(issued.getTime() + 60_000), expectNonce: "abc123", site: "exchange.example" });
  assert.equal(ok.nonce, "abc123");
});

test("stale, foreign, tampered and mis-sited challenges are refused", () => {
  const key = Keypair.fromSeed(6n);
  const other = Keypair.fromSeed(7n);
  const issued = new Date("2026-09-27T12:00:00Z");
  const msg = loginChallenge(key.address, { nonce: "n1", issued });
  const { signature } = signLoginWith(key, msg);
  assert.throws(() => verifyLoginChallenge(key.address, msg, signature, { now: new Date(issued.getTime() + 10 * 60_000) }), /expired/);
  assert.throws(() => verifyLoginChallenge(other.address, msg, signature, { now: issued }), /another address/);
  assert.throws(() => verifyLoginChallenge(key.address, msg.replace("n1", "n2"), signature, { now: issued }), /bad signature/);
  assert.throws(() => verifyLoginChallenge(key.address, msg, signature, { now: issued, expectNonce: "zz" }), /nonce/);
  assert.throws(() => verifyLoginChallenge(key.address, msg, signature, { now: issued, site: "a.example" }), /another site/);
  assert.equal(parseLoginChallenge("hello"), null);
});
