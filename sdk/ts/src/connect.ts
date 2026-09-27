// Connecting a site to Keel Wallet (docs/connect-keel-wallet.md).
//
// Browser side: find the provider the extension injects, connect, ask for a
// login signature or a session key, or drop a ready-made button into the
// page. Server side: `verifyLoginChallenge` checks the signature, the
// address and the age of a challenge the server issued.
import { Keypair, signMessage, verifyMessage } from "./sign.ts";

export type SessionScope = "markets" | "p2p_manage";

export interface AccountInfo { address: string; network: string; chainId: number }

/** The `window.keel` provider (apps/wallet-extension/src/inpage/types.ts). */
export interface KeelProvider {
  readonly isKeel: true;
  readonly version: string;
  connect(opts?: { network?: string }): Promise<AccountInfo>;
  disconnect(): Promise<void>;
  getAccount(): Promise<AccountInfo | null>;
  signMessage(message: string): Promise<{ address: string; signature: string }>;
  signAction(req: { envelope: { signer: string; nonce: number; chain_id: number; action: unknown }; context?: { title?: string; description?: string } }): Promise<{ signature: string; tx_id: string; signed: unknown }>;
  authorizeSession(req: { key: string; scope: SessionScope[]; expires_at: number; nonce: number; chain_id: number }): Promise<{ signature: string; tx_id: string; signed: unknown }>;
  on(event: "accountChanged" | "disconnect" | "networkChanged", handler: (payload: unknown) => void): () => void;
}

type WithKeel = { keel?: KeelProvider };
const win = (): WithKeel | null => (typeof window === "undefined" ? null : (window as unknown as WithKeel));

/** The injected provider, waiting up to `timeoutMs` for the extension to load. */
export function detectProvider(timeoutMs = 1500): Promise<KeelProvider | null> {
  const w = win();
  if (!w) return Promise.resolve(null);
  if (w.keel?.isKeel) return Promise.resolve(w.keel);
  return new Promise((resolve) => {
    const done = () => { cleanup(); resolve(w.keel?.isKeel ? w.keel : null); };
    const timer = setTimeout(done, timeoutMs);
    const cleanup = () => { clearTimeout(timer); window.removeEventListener("keel#initialized", done); };
    window.addEventListener("keel#initialized", done, { once: true });
  });
}

export class NoWalletError extends Error {
  constructor() { super("Keel Wallet is not installed or not enabled on this site"); this.name = "NoWalletError"; }
}

/** Connect (the wallet asks the user once per site and network). */
export async function connectWallet(opts: { network?: string; timeoutMs?: number } = {}): Promise<{ provider: KeelProvider; account: AccountInfo }> {
  const provider = await detectProvider(opts.timeoutMs);
  if (!provider) throw new NoWalletError();
  const account = await provider.connect(opts.network ? { network: opts.network } : undefined);
  return { provider, account };
}

// ---------------------------------------------------------------- login challenge

/** Text the server issues and the wallet signs (docs/wallet.md §3). */
export function loginChallenge(address: string, opts: { nonce?: string; issued?: Date; site?: string } = {}): string {
  const nonce = opts.nonce ?? randomHex(16);
  const issued = (opts.issued ?? new Date()).toISOString();
  const lines = ["Keel login", `address: ${address.toLowerCase()}`, `nonce: ${nonce}`, `issued: ${issued}`];
  if (opts.site) lines.push(`site: ${opts.site}`);
  return lines.join("\n");
}

export interface ParsedChallenge { address: string; nonce: string; issued: Date; site?: string }

export function parseLoginChallenge(message: string): ParsedChallenge | null {
  const lines = message.split("\n");
  if (lines[0] !== "Keel login") return null;
  const fields: Record<string, string> = {};
  for (const l of lines.slice(1)) {
    const i = l.indexOf(": ");
    if (i < 0) return null;
    fields[l.slice(0, i)] = l.slice(i + 2);
  }
  const address = fields["address"];
  const nonce = fields["nonce"];
  const issued = fields["issued"] ? new Date(fields["issued"]) : null;
  if (!address || !/^[0-9a-f]{64}$/.test(address) || !nonce || !issued || Number.isNaN(issued.getTime())) return null;
  return { address, nonce, issued, ...(fields["site"] ? { site: fields["site"] } : {}) };
}

/**
 * Server side: the signature is the address's, the challenge names that
 * address, was issued within `maxAgeMs`, and (when the server remembers
 * its nonces) `expectNonce` matches. Returns the parsed challenge.
 */
export function verifyLoginChallenge(
  address: string,
  message: string,
  signatureHex: string,
  opts: { maxAgeMs?: number; now?: Date; expectNonce?: string; site?: string } = {},
): ParsedChallenge {
  const parsed = parseLoginChallenge(message);
  if (!parsed) throw new Error("not a Keel login challenge");
  if (parsed.address !== address.toLowerCase()) throw new Error("challenge names another address");
  const now = opts.now ?? new Date();
  const age = now.getTime() - parsed.issued.getTime();
  if (age < -60_000 || age > (opts.maxAgeMs ?? 5 * 60_000)) throw new Error("challenge expired");
  if (opts.expectNonce !== undefined && opts.expectNonce !== parsed.nonce) throw new Error("unexpected nonce");
  if (opts.site !== undefined && parsed.site !== opts.site) throw new Error("challenge is for another site");
  if (!verifyMessage(address, message, signatureHex)) throw new Error("bad signature");
  return parsed;
}

/** Browser side: ask the wallet to sign a challenge the server issued. */
export async function signLogin(provider: KeelProvider, challenge: string): Promise<{ address: string; signature: string }> {
  return provider.signMessage(challenge);
}

/** Test helper: what a wallet would produce for `challenge`. */
export function signLoginWith(key: Keypair, challenge: string): { address: string; signature: string } {
  return { address: key.address, signature: signMessage(key.secret, challenge) };
}

// ---------------------------------------------------------------- session keys

/**
 * Ask the wallet to authorize a fresh session key for `scopes`. The site
 * keeps the returned secret (server side, per user) and signs
 * markets / offer actions with it until `expiresAt`; a session key can
 * never move funds.
 */
export async function requestSession(
  provider: KeelProvider,
  opts: { scopes: SessionScope[]; nonce: number; chainId: number; days?: number },
): Promise<{ key: Keypair; expiresAt: number; txId: string; signed: unknown }> {
  const key = Keypair.random();
  const expiresAt = Math.floor(Date.now() / 1000) + Math.min(opts.days ?? 30, 30) * 86_400;
  const r = await provider.authorizeSession({ key: key.address, scope: opts.scopes, expires_at: expiresAt, nonce: opts.nonce, chain_id: opts.chainId });
  return { key, expiresAt, txId: r.tx_id, signed: r.signed };
}

// ---------------------------------------------------------------- a button

export interface ConnectButtonOptions {
  label?: string;
  connectedLabel?: (a: AccountInfo) => string;
  installUrl?: string;
  network?: string;
  onConnected?: (a: AccountInfo, p: KeelProvider) => void;
  onError?: (e: Error) => void;
}

/** A plain button that connects, without any framework: `mountConnectButton(document.getElementById("keel"))`. */
export function mountConnectButton(el: HTMLElement, opts: ConnectButtonOptions = {}): () => void {
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "keel-connect";
  btn.textContent = opts.label ?? "Connect Keel Wallet";
  const short = (a: string) => `${a.slice(0, 6)}…${a.slice(-4)}`;
  let off: (() => void) | null = null;
  btn.onclick = async () => {
    btn.disabled = true;
    try {
      const { provider, account } = await connectWallet({ network: opts.network });
      btn.textContent = (opts.connectedLabel ?? ((a) => `Connected ${short(a.address)}`))(account);
      off?.();
      off = provider.on("accountChanged", (p) => {
        const a = p as AccountInfo;
        btn.textContent = (opts.connectedLabel ?? ((x) => `Connected ${short(x.address)}`))(a);
        opts.onConnected?.(a, provider);
      });
      opts.onConnected?.(account, provider);
    } catch (e) {
      if (e instanceof NoWalletError && opts.installUrl) {
        window.open(opts.installUrl, "_blank", "noopener");
      }
      opts.onError?.(e instanceof Error ? e : new Error(String(e)));
      btn.textContent = opts.label ?? "Connect Keel Wallet";
    } finally {
      btn.disabled = false;
    }
  };
  el.appendChild(btn);
  return () => { off?.(); btn.remove(); };
}

function randomHex(bytes: number): string {
  const out = new Uint8Array(bytes);
  const c = (globalThis as unknown as { crypto?: { getRandomValues(a: Uint8Array): Uint8Array } }).crypto;
  if (c?.getRandomValues) c.getRandomValues(out);
  else for (let i = 0; i < bytes; i++) out[i] = Math.floor(Math.random() * 256);
  return Array.from(out, (b) => b.toString(16).padStart(2, "0")).join("");
}
