// KeelSocket over a scripted fake WebSocket: subscribe handshake, sequence
// tracking, synthetic gaps, and re-subscribe from the last height after a
// drop.
import { test } from "node:test";
import assert from "node:assert/strict";
import { KeelSocket } from "../src/ws.ts";

class FakeWs {
  static instances: FakeWs[] = [];
  readyState = 0;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  seq = 0;
  url: string;
  constructor(url: string) {
    this.url = url;
    FakeWs.instances.push(this);
    setTimeout(() => { this.readyState = 1; this.onopen?.(); }, 0);
  }
  send(s: string) {
    this.sent.push(s);
    const m = JSON.parse(s);
    if (m.op === "subscribe") this.push({ type: "subscribed", channels: m.channels, height: 10 });
    if (m.op === "ping") this.push({ type: "pong", height: 10 });
  }
  close() { this.readyState = 3; this.onclose?.(); }
  push(frame: Record<string, unknown>) {
    this.seq += 1;
    this.onmessage?.({ data: JSON.stringify({ seq: this.seq, ...frame }) });
  }
}

const tick = () => new Promise((r) => setTimeout(r, 5));
const impl = FakeWs as unknown as typeof WebSocket;

test("subscribe is acknowledged and frames are routed by type and channel", async () => {
  FakeWs.instances = [];
  const ws = new KeelSocket("ws://x/v1/ws", { WebSocketImpl: impl, reconnect: false });
  const ack = await ws.subscribe(["account:ab", "blocks"]);
  assert.equal(ack.type, "subscribed");
  const seen: string[] = [];
  ws.on("event", (f) => seen.push(`type:${f.channel}`));
  ws.on("account:ab", (f) => seen.push(`channel:${f.type}`));
  FakeWs.instances[0].push({ type: "event", channel: "account:ab", height: 11, data: {} });
  assert.deepEqual(seen, ["type:account:ab", "channel:event"]);
  assert.equal(ws.height, 11);
  ws.close();
});

test("a sequence hole raises a synthetic gap", async () => {
  FakeWs.instances = [];
  const ws = new KeelSocket("ws://x/v1/ws", { WebSocketImpl: impl, reconnect: false });
  await ws.subscribe(["blocks"]);
  const gaps: number[] = [];
  ws.on("gap", (f) => gaps.push(Number(f.missed)));
  const fake = FakeWs.instances[0];
  fake.push({ type: "block", channel: "blocks", height: 11 });
  fake.seq += 3; // three frames lost somewhere
  fake.push({ type: "block", channel: "blocks", height: 15 });
  assert.deepEqual(gaps, [3]);
  ws.close();
});

test("after a drop the client reconnects and re-subscribes from the last height", async () => {
  FakeWs.instances = [];
  const ws = new KeelSocket("ws://x/v1/ws", { WebSocketImpl: impl, backoffMs: 1 });
  await ws.subscribe(["account:ab"]);
  FakeWs.instances[0].push({ type: "event", channel: "account:ab", height: 42, data: {} });
  FakeWs.instances[0].close();
  await tick(); await tick();
  assert.equal(FakeWs.instances.length, 2, "reconnected");
  const resub = JSON.parse(FakeWs.instances[1].sent[0]);
  assert.equal(resub.op, "subscribe");
  assert.deepEqual(resub.channels, ["account:ab"]);
  assert.equal(resub.from_height, 43);
  ws.close();
});
