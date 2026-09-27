// Subscriptions over the node's or the indexer's /v1/ws (crates/keel-rpc/API-WS.md).
//
//   const ws = new KeelSocket("wss://testnet.keelchain.com/rpc/v1/ws");
//   ws.on("event", (m) => console.log(m.channel, m.data));
//   ws.on("gap", (m) => resync(m.from_height, m.to_height));
//   await ws.subscribe([`account:${me}`, "book:BTC-KUSD?depth=10"]);
//
// Every server frame carries `seq` (per connection, monotonic) and `height`.
// The client checks `seq` and reports a hole as a synthetic `gap`; on a
// dropped connection it reconnects with backoff and re-subscribes from the
// last height it saw, so event channels replay what was missed.

export type Frame = {
  type: string;
  seq: number;
  height: number;
  channel?: string;
  [k: string]: unknown;
};

export type FrameHandler = (frame: Frame) => void;

export interface KeelSocketOptions {
  /** Channels to subscribe as soon as the socket opens. */
  channels?: string[];
  /** Replay event channels from this height on the first subscribe. */
  fromHeight?: number;
  /** Reconnect on close (default true). */
  reconnect?: boolean;
  /** First reconnect delay in ms (doubles up to 30 s). */
  backoffMs?: number;
  /** WebSocket constructor (defaults to the global one). */
  WebSocketImpl?: typeof WebSocket;
}

export class KeelSocket {
  readonly url: string;
  private ws: WebSocket | null = null;
  private readonly handlers = new Map<string, Set<FrameHandler>>();
  private channels: string[];
  private fromHeight: number | undefined;
  private lastSeq = 0;
  private lastHeight = 0;
  private closed = false;
  private backoff: number;
  private readonly opts: Required<Pick<KeelSocketOptions, "reconnect" | "backoffMs">> & { WebSocketImpl: typeof WebSocket };
  private pending: Array<() => void> = [];

  constructor(url: string, opts: KeelSocketOptions = {}) {
    this.url = url;
    this.channels = opts.channels ?? [];
    this.fromHeight = opts.fromHeight;
    this.opts = {
      reconnect: opts.reconnect ?? true,
      backoffMs: opts.backoffMs ?? 500,
      WebSocketImpl: opts.WebSocketImpl ?? (globalThis as unknown as { WebSocket: typeof WebSocket }).WebSocket,
    };
    this.backoff = this.opts.backoffMs;
    this.connect();
  }

  /** Last `height` any frame reported. */
  get height(): number { return this.lastHeight; }

  /** Listen to a frame type ("event", "block", "gap", "heartbeat", ...) or to a channel name. */
  on(key: string, handler: FrameHandler): () => void {
    let set = this.handlers.get(key);
    if (!set) { set = new Set(); this.handlers.set(key, set); }
    set.add(handler);
    return () => { this.handlers.get(key)?.delete(handler); };
  }

  /** Replace the channel set; resolves when the server acknowledges. */
  subscribe(channels: string[], fromHeight?: number): Promise<Frame> {
    this.channels = channels;
    if (fromHeight !== undefined) this.fromHeight = fromHeight;
    return this.send({ op: "subscribe", channels, ...(this.fromHeight !== undefined ? { from_height: this.fromHeight } : {}) }, "subscribed");
  }

  unsubscribe(channels: string[]): Promise<Frame> {
    this.channels = this.channels.filter((c) => !channels.includes(c));
    return this.send({ op: "unsubscribe", channels }, "unsubscribed");
  }

  ping(): Promise<Frame> { return this.send({ op: "ping" }, "pong"); }

  close(): void {
    this.closed = true;
    this.ws?.close();
    this.ws = null;
  }

  private send(msg: unknown, ack: string): Promise<Frame> {
    return new Promise((resolve, reject) => {
      const run = () => {
        const ws = this.ws;
        if (!ws || ws.readyState !== 1) { this.pending.push(run); return; }
        const off = this.on(ack, (f) => { off(); resolve(f); });
        const offErr = this.on("error", (f) => { off(); offErr(); reject(new Error(String(f.message))); });
        ws.send(JSON.stringify(msg));
      };
      run();
    });
  }

  private emit(key: string, frame: Frame): void {
    for (const h of Array.from(this.handlers.get(key) ?? [])) {
      try { h(frame); } catch { /* one listener's error must not break the others */ }
    }
  }

  private connect(): void {
    if (this.closed) return;
    const ws = new this.opts.WebSocketImpl(this.url);
    this.ws = ws;
    ws.onopen = () => {
      this.backoff = this.opts.backoffMs;
      this.lastSeq = 0;
      if (this.channels.length > 0) {
        const from = this.fromHeight ?? (this.lastHeight > 0 ? this.lastHeight + 1 : undefined);
        ws.send(JSON.stringify({ op: "subscribe", channels: this.channels, ...(from !== undefined ? { from_height: from } : {}) }));
        this.fromHeight = undefined;
      }
      const queued = this.pending; this.pending = [];
      for (const run of queued) run();
      this.emit("open", { type: "open", seq: 0, height: this.lastHeight });
    };
    ws.onmessage = (ev: MessageEvent) => {
      let frame: Frame;
      try { frame = JSON.parse(String(ev.data)); } catch { return; }
      if (typeof frame.seq === "number") {
        if (this.lastSeq > 0 && frame.seq !== this.lastSeq + 1) {
          this.emit("gap", { type: "gap", seq: frame.seq, height: frame.height, from_height: this.lastHeight + 1, to_height: frame.height, missed: frame.seq - this.lastSeq - 1, synthetic: true });
        }
        this.lastSeq = frame.seq;
      }
      if (typeof frame.height === "number" && frame.height > this.lastHeight) this.lastHeight = frame.height;
      this.emit(frame.type, frame);
      if (frame.channel) this.emit(frame.channel, frame);
      this.emit("*", frame);
    };
    ws.onclose = () => {
      this.ws = null;
      this.emit("close", { type: "close", seq: this.lastSeq, height: this.lastHeight });
      if (this.closed || !this.opts.reconnect) return;
      const delay = this.backoff;
      this.backoff = Math.min(this.backoff * 2, 30_000);
      setTimeout(() => this.connect(), delay);
    };
    ws.onerror = () => { /* onclose follows */ };
  }
}
