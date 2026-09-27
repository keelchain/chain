// Minimal borsh writer: exactly the subset keel-actions uses.
export class Writer {
  private buf = new Uint8Array(256);
  private len = 0;

  private ensure(n: number): void {
    if (this.len + n <= this.buf.length) return;
    let cap = this.buf.length * 2;
    while (cap < this.len + n) cap *= 2;
    const next = new Uint8Array(cap);
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
  }

  bytes(): Uint8Array {
    return this.buf.slice(0, this.len);
  }

  u8(v: number): void {
    this.ensure(1);
    this.buf[this.len++] = v & 0xff;
  }

  bool(v: boolean): void {
    this.u8(v ? 1 : 0);
  }

  private uint(v: bigint | number, bytes: number): void {
    let x = BigInt(v);
    if (x < 0n) throw new Error("negative unsigned");
    this.ensure(bytes);
    for (let i = 0; i < bytes; i++) {
      this.buf[this.len++] = Number(x & 0xffn);
      x >>= 8n;
    }
    if (x !== 0n) throw new Error(`value does not fit in u${bytes * 8}`);
  }

  u16(v: number): void { this.uint(v, 2); }
  u32(v: number): void { this.uint(v, 4); }
  u64(v: bigint | number | string): void { this.uint(typeof v === "string" ? BigInt(v) : v, 8); }
  u128(v: bigint | number | string): void { this.uint(typeof v === "string" ? BigInt(v) : v, 16); }

  i32(v: number): void {
    this.ensure(4);
    new DataView(this.buf.buffer, this.buf.byteOffset + this.len, 4).setInt32(0, v, true);
    this.len += 4;
  }

  string(s: string): void {
    const b = new TextEncoder().encode(s);
    this.u32(b.length);
    this.raw(b);
  }

  /// Vec<u8>
  vecU8(b: Uint8Array): void {
    this.u32(b.length);
    this.raw(b);
  }

  /// [u8; N]
  fixed(b: Uint8Array, n: number): void {
    if (b.length !== n) throw new Error(`expected ${n} bytes, got ${b.length}`);
    this.raw(b);
  }

  raw(b: Uint8Array): void {
    this.ensure(b.length);
    this.buf.set(b, this.len);
    this.len += b.length;
  }

  option<T>(v: T | null | undefined, write: (x: T) => void): void {
    if (v === null || v === undefined) this.u8(0);
    else {
      this.u8(1);
      write(v);
    }
  }

  vec<T>(items: readonly T[], write: (x: T) => void): void {
    this.u32(items.length);
    for (const it of items) write(it);
  }
}

export function hexToBytes(hex: string): Uint8Array {
  const h = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (h.length % 2 !== 0) throw new Error("odd hex length");
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToHex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}
