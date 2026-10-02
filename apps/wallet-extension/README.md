# Keel Wallet (browser extension)

Non-custodial Manifest V3 wallet for the Keelchain. Keys are generated and
kept inside the extension; web apps ask for signatures through the injected
`window.keel` provider and every signature goes through an approval screen.
Contract: `docs/wallet.md`. Built on `sdk/ts` (`@keelchain/sdk`).

## Build, test, load

```sh
cd sdk/ts && npm install && npm run build      # the SDK dist the wallet links to (file:../../sdk/ts)
cd apps/wallet-extension
npm install
npm test          # vitest
npm run build     # tsc --noEmit + scripts/build.mjs → dist/
```

`dist/` is a complete unpacked extension: `manifest.json`, `background.js`
(service worker), `content.js` (isolated world bridge), `inpage.js` (main
world, defines `window.keel`), `popup.html` + `assets/`, `icons/`.

Load it unpacked:

- **Chrome / Brave / Edge (≥ 121)**: `chrome://extensions` → enable
  *Developer mode* → *Load unpacked* → pick `apps/wallet-extension/dist`.
- **Firefox (≥ 128)**: `about:debugging#/runtime/this-firefox` → *Load
  Temporary Add-on…* → pick `dist/manifest.json`.

Reload the extension after every `npm run build`; reload open tabs so the
content scripts are re-injected.

## Key derivation

```
mnemonic  = BIP39, English wordlist, 24 words (256 bits of entropy)
seed      = BIP39-seed(mnemonic, passphrase = "")          # PBKDF2-HMAC-SHA512, 2048 rounds, 64 bytes
secret_0  = seed[0..32]                                    # account 0: first 32 bytes of the seed (docs/wallet.md)
secret_i  = sha256(seed ‖ u32_le(i))         for i ≥ 1     # 64-byte seed followed by the index as 4 little-endian bytes
address_i = ed25519_public_key(secret_i)                   # RFC 8032 key from the 32-byte secret; hex, lowercase
```

Any BIP39 tool + Ed25519 implementation recovers the same keys. No BIP32/
SLIP-10 path is used. Test vector (pinned in `src/core/keys.test.ts`):

| | |
|---|---|
| phrase | `abandon` × 23 + `art` |
| seed | `408b285c123836004f4b8842c89324c1f01382450c0d439af345ba7fc49acf705489c6fc77dbd4e3dc1dd8cc6bc9f043db8ada1e243c4a0eafb290d399480840` |
| secret_0 | `408b285c123836004f4b8842c89324c1f01382450c0d439af345ba7fc49acf70` |
| **address_0** | `1de352e44cd333672593f2334a730e180aaf290de89aa16d480de594e34e2961` |
| secret_1 | `d08127fabcc24eb0b83b2f129c9c31c06109e211946301fac107f7b7125802e4` |
| address_1 | `59424260f06f5414dec45bd8b9354bef75425bdc79c40f70d38e838e8831ff97` |

### Vault

The mnemonic is the only secret at rest. It is stored in
`chrome.storage.local` as
`{ version: 1, kdf: { name: "PBKDF2-SHA256", iterations: 310000, salt }, cipher: { name: "AES-GCM", iv }, ciphertext }`
(hex fields; 16-byte random salt, 12-byte random IV, 256-bit key derived
with WebCrypto from the NFKC-normalised password). Account names, addresses,
the selected network and connected origins are stored unencrypted (they are
public data).

### Signatures

- Actions: `signer(32) ‖ nonce(u64 LE) ‖ chain_id(u32 LE) ‖ borsh(action)`,
  digest `sha256("keel-action-v1" ‖ envelope)`, Ed25519 over the digest,
  `tx_id = sha256("keel-txid" ‖ digest ‖ signature)` — all via `@keelchain/sdk`
  (`Keypair.sign`, `txId`, `verify`).
- Messages: digest `sha256("keel-message-v1" ‖ utf8(message))`, Ed25519 over
  the digest, hex — via `@keelchain/sdk` `signMessage` / `verifyMessage`. The
  domain tag makes a message signature unusable as a chain action.

## Provider API (`window.keel`)

Types: `src/inpage/types.ts` (self-contained; copy it into the web app).
The script is injected on Keelchain's own sites and localhost at `document_start` and
fires `keel#initialized` on `window`.

```ts
interface KeelProvider {
  readonly isKeel: true;
  readonly version: '1.0.0';
  connect(opts?: { network?: string }): Promise<{ address: string; network: string; chainId: number }>;
  disconnect(): Promise<void>;
  getAccount(): Promise<{ address: string; network: string; chainId: number } | null>;   // never prompts
  signMessage(message: string): Promise<{ address: string; signature: string }>;
  signAction(req: { envelope: { signer: string; nonce: number; chain_id: number; action: Action };
                    context?: { title?: string; description?: string } })
    : Promise<{ signature: string; tx_id: string; signed: SignedAction }>;
  authorizeSession(req: { key: string; scope: ('markets' | 'p2p_manage')[]; expires_at: number; nonce: number; chain_id: number })
    : Promise<{ signature: string; tx_id: string; signed: SignedAction }>;
  on(event: 'accountChanged' | 'disconnect' | 'networkChanged', handler: (payload: unknown) => void): () => void;
}
```

Rejections are `Error` objects that also satisfy `{ code, message }`:

| code | when |
|---|---|
| `USER_REJECTED` | the user pressed Reject, or closed the approval window while unlocked |
| `LOCKED` | the approval window was closed without unlocking |
| `NO_ACCOUNT` | no wallet has been created/imported yet |
| `NOT_CONNECTED` | `signMessage` / `signAction` / `authorizeSession` from an origin that has not called `connect()` on the current network |
| `INVALID_REQUEST` | malformed params, `envelope.signer` ≠ connected address, action that does not borsh-encode, bad session scope/expiry (must be in the future, ≤ 30 days) |
| `WRONG_NETWORK` | `connect({network})` names a network other than the wallet's, `envelope.chain_id` ≠ the wallet network's chain id, or the wallet is on the mainnet placeholder |

Behaviour:

- `connect()` opens an approval once per origin **per network**; later calls
  return immediately. `disconnect()` forgets the origin and emits
  `disconnect`.
- `getAccount()` answers from storage, even while locked, and never opens a
  popup.
- Every other call opens the approval window. If the wallet is locked the
  window first asks for the password, then shows the request.
- `signAction` shows origin, optional caller context, the decoded action
  (kind, plain-words summary, fields with asset decimals: KEEL 6, KUSD 6,
  BTC.BTC 8, ETH.ETH 18, ETH.USDT 6, TRON.USDT 6, TRON.TRX 6; pairs are
  `BASE-QUOTE`), a fund-movement warning where relevant, the raw action
  JSON on demand, nonce, signer and network, then Reject / Approve.
- `authorizeSession` builds `{ AuthorizeSessionKey: { key, scope: bits, expires_at } }`
  (bits: `markets = 1`, `p2p_manage = 2`) and shows *"Allow &lt;origin&gt; to
  place and cancel orders for you until &lt;date&gt;. It can never move funds."*
- `signMessage` shows the message text verbatim.
- Events: `accountChanged` (`AccountInfo | null`) when the active account or
  network changes, `networkChanged` (`{ network, chainId }`), `disconnect`
  (`{ network }`). Only origins that are connected receive them.
- `bigint` values in an action are preserved across the boundary (they are
  carried as `{ "$bigint": "…" }` markers and restored on both sides), so
  `signed.envelope.action` equals what you passed.

### Message protocol (for reference)

`inpage → content`: `window.postMessage({ channel: 'keel-wallet-v1', dir: 'to-wallet', request: { id, method, params } })`.
`content → background`: `chrome.runtime.sendMessage({ kind: 'keel:provider', request })`; the background takes the
origin from `sender`, never from the payload.
`background → content → inpage`: `{ id, ok: true, result } | { id, ok: false, error: { code, message } }`, and
`{ kind: 'keel:event', event: { event, payload } }` for events.

## Networks

| id | rpc | chain id | explorer |
|---|---|---|---|
| `testnet` | `http://127.0.0.1:5000` | 1 | `http://127.0.0.1:5177/testnet` |
| `mainnet` | placeholder (not launched; signing refused with `WRONG_NETWORK`) | 0 | – |

The list lives in `src/core/networks.ts`; `host_permissions` in
`manifest.json` covers `127.0.0.1` / `localhost` so the popup can read
`GET <rpc>/v1/accounts/<address>` for balances. That is the wallet's only
network call.

## Security notes

- Private keys never leave the background service worker. The popup, the
  content script and the page only ever see addresses, signatures and
  decoded previews. Signing is done in the worker after the user approves.
- Unlocked secrets (the 64-byte seed and derived keypairs) live in worker
  memory only and are zeroed on lock. Auto-lock is inactivity based
  (default 15 minutes, configurable in Settings). The browser may evict an
  idle service worker earlier; that also drops the keys, so you may be asked
  for the password sooner than the timer. Nothing unlocked is ever written
  to storage.
- Nothing is signed silently: every `signMessage`, `signAction` and
  `authorizeSession` requires a click. Session keys never receive
  fund-moving scopes; the chain enforces that too.
- Origins are taken from the browser's `sender` metadata, and only
  `http`/`https` page origins are served. The popup's own messages are
  accepted only from extension pages.
- The wallet makes no network requests other than the configured node RPC
  (balance display); never to the marketplace.
- The extension's pages run with `script-src 'self'` and no remote code.
- Writing the seed phrase down is the only backup. *Reset wallet* erases the
  vault after confirming the password.

## Layout

```
manifest.json            MV3 manifest (Chrome/Brave/Edge/Firefox)
popup.html               popup entry (Vite)
scripts/build.mjs        4 Vite builds → dist/ (+ manifest, generated icons)
src/inpage/types.ts      public provider types (copy into web apps)
src/inpage/provider.ts   window.keel implementation over a Transport
src/inpage/index.ts      main-world entry
src/content/index.ts     isolated-world bridge page ↔ background
src/background/index.ts  service worker: chrome.* wiring, approval window
src/background/wallet.ts controller: provider + popup requests (browser-free, tested)
src/background/session.ts unlocked keys + auto-lock
src/background/approvals.ts pending approvals queue
src/background/state.ts  persisted non-secret state
src/core/vault.ts        PBKDF2-SHA256 + AES-GCM vault
src/core/keys.ts         BIP39 + account derivation
src/core/sign.ts         message/action signing via @keelchain/sdk
src/core/actions.ts      envelope/session validation, scope bits
src/core/decode.ts       action → human-readable view
src/core/format.ts       amounts, decimals, pairs
src/core/networks.ts     network list
src/core/protocol.ts     message types, error codes, bigint-safe JSON
src/popup/               React UI (onboarding, unlock, home, settings, approvals)
```

## Store packages

`npm run package` builds `dist/` and writes `dist-store/`: a Chromium zip
(Chrome Web Store, Edge Add-ons, Brave, Opera), a Firefox zip (AMO, add-on
id `keel-wallet@keelchain.com`, Firefox ≥ 140) and the source archive AMO asks for.
Listing texts, permission justifications and the per-store checklist are in
`STORE.md`; the privacy policy page is `public/wallet-privacy.html`, served
at https://keelchain.com/wallet/privacy.html. The default
network is the public Keel testnet (chain id 3).

`scripts/e2e-signup.mjs` drives a real signup-with-wallet flow against a client site (`E2E_SITE`) in Playwright's Chromium and produces the store screenshots (see STORE.md).

## Sites, sending and networks (1.2.0)

- **Any site.** The manifest injects the provider on keelchain.com and
  localhost only. For any other site the user opens the wallet's settings
  and clicks *Enable on this site* (or types the site's URL): the browser
  asks for that origin once (`optional_host_permissions`), and the
  background registers the content scripts for it with
  `chrome.scripting.registerContentScripts` (persisted, re-registered on
  update). Removing a site drops the permission. No release per client.
  Since 1.2.1 the popup's home screen offers the same button whenever it
  is opened on a site that is not enabled yet, and enabling also puts the
  provider on the page that is already open, so there is nothing to
  reload: a new user installs, opens the wallet on the site, clicks once
  and carries on with the site's sign-up.
- **Send.** The home page sends a `Transfer` (to a Keel address) or a
  `Withdraw` (to an external address) from the active account, with a
  review step; the wallet signs and submits to the network's RPC and waits
  for the receipt. No site is involved, so the form is the approval.
- **Custom networks.** Settings lets the user add a network by name and RPC
  URL; the chain id is read from `/v1/status`. Custom networks are stored
  with the wallet state and offered in the network switcher.
- **Updates.** The Firefox build is signed as an unlisted add-on by
  `.github/workflows/publish-wallet.yml` and served from
  keelchain.com/wallet/downloads with an `updates.json`; the manifest's
  `update_url` points there.
