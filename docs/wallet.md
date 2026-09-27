# Keel Wallet: non-custodial keys (decided 2026-09-09)

My requirement: no user private key on any server, ever. The user
holds the key in a browser extension (later a mobile app); the marketplace
becomes a front end that asks the wallet to sign. This document is the
contract between the three parts:

- `apps/wallet-extension` — the browser wallet (MV3), built on `sdk/ts`.
- `apps/marketplace` — asks for signatures, never holds user keys
  (custodial keys remain only until each user migrates).
- `apps/web` — the site: connects the wallet, relays signing requests.

Chain-side support: session keys (`AuthorizeSessionKey` /
`RevokeSessionKey`) so order-book trading does not need a popup per click,
and permissionless trade expiry so a stuck trade needs no operator key.

## 1. Keys and signatures (unchanged chain primitives)

- Account key: Ed25519. The chain address is the 32-byte public key (hex).
- Action envelope: `signer(32) ‖ nonce(u64) ‖ chain_id(u32) ‖ borsh(action)`;
  digest `sha256("keel-action-v1" ‖ envelope)`; signature Ed25519 over the
  digest; `tx_id = sha256("keel-txid" ‖ digest ‖ signature)`. All in
  `sdk/ts/src/sign.ts` (byte-identical to `crates/keel-actions`).
- Message signing (login challenges, wallet linking), NOT a chain action:
  digest `sha256("keel-message-v1" ‖ utf8(message))`, Ed25519 signature,
  hex. A message can never be mistaken for an envelope because the domain
  tags differ.

## 2. Provider API — `window.keel`

Injected by the extension's content script on Keelchain's own sites and on
localhost out of the box, and on any site the user enables from the wallet's
settings (the browser asks for that origin once; no wallet release is needed
per client). `connect-keel-wallet.md` is the integration guide. All methods
return promises; rejections use `{ code, message }` with codes
`USER_REJECTED`, `LOCKED`, `NO_ACCOUNT`, `NOT_CONNECTED`, `INVALID_REQUEST`,
`WRONG_NETWORK`.

```ts
interface KeelProvider {
  readonly isKeel: true;
  readonly version: string;                      // "1.0.0"

  /** Ask the user to connect this origin. Remembered per origin until disconnect. */
  connect(opts?: { network?: string }): Promise<{ address: string; network: string; chainId: number }>;
  disconnect(): Promise<void>;
  /** The connected account for this origin, or null (never prompts). */
  getAccount(): Promise<{ address: string; network: string; chainId: number } | null>;

  /** Sign an arbitrary message (login challenge). Popup shows the text. */
  signMessage(message: string): Promise<{ address: string; signature: string }>;

  /**
   * Sign a chain action. The popup decodes the action (SDK) and shows a
   * human-readable summary before the user approves. `envelope.signer`
   * must equal the connected address, else INVALID_REQUEST.
   */
  signAction(req: {
    envelope: { signer: string; nonce: number; chain_id: number; action: Action };
    /** Optional caller context shown above the decoded action. */
    context?: { title?: string; description?: string };
  }): Promise<{ signature: string; tx_id: string; signed: SignedAction }>;

  /** Sugar for signAction with `AuthorizeSessionKey`; the popup explains the scope and expiry. */
  authorizeSession(req: { key: string; scope: SessionScope[]; expires_at: number; nonce: number; chain_id: number }): Promise<{ signature: string; tx_id: string; signed: SignedAction }>;

  on(event: 'accountChanged' | 'disconnect' | 'networkChanged', handler: (payload: unknown) => void): () => void;
}
type SessionScope = 'markets' | 'p2p_manage';
```

Extension behaviour:

- One vault (encrypted with the wallet password, PBKDF2-SHA256 ≥ 300k
  iterations + AES-GCM) holding one or more Ed25519 keys; the seed phrase
  is BIP39 (24 words), key = first 32 bytes of the BIP39 seed (documented,
  so any BIP39 tool can recover the same key).
- Networks: a list `{ id, name, rpc, chainId, explorer }`; default entries
  `testnet` (`http://127.0.0.1:5000`, chain id 1, explorer
  `http://127.0.0.1:5177/testnet`) and `mainnet` (placeholder).
- Every `signAction` shows: origin, decoded action (kind, fields with asset
  decimals), nonce, network, and a "Reject / Approve" pair. Nothing is
  signed silently. `signMessage` shows the message.
- Connected origins are stored per network; `getAccount` answers without
  a prompt; anything else while locked prompts for the password.
- No network calls except the node RPC the user configured (balance
  display), never to the marketplace.

## 3. Marketplace API (the site talks to this)

### Login and linking

| Method | Path | Body → Result |
|---|---|---|
| POST | `/v1/auth/wallet/challenge` | `{address}` → `{challenge, expiresAt}`; challenge text: `Keel login\naddress: <addr>\nnonce: <random>\nissued: <iso>` |
| POST | `/v1/auth/wallet/login` | `{address, challenge, signature}` → the normal session `{token, userId, role, walletAddress}` for an address already bound to an account |
| POST | `/v1/auth/wallet/register` | `{address, challenge, signature, email, country, referralCode?, attribution?}` → 201 with the session; creates the account with the wallet as its credential (no password; the email is still required for notifications, verification and KYC) and binds the address immediately |
| POST | `/v1/auth/wallet/link` | logged-in email user; `{address, signature}` → binds the address to the account (starts migration for custodial users, see §5) |

### Signing requests (server needs a signature from the wallet)

The server keeps every existing flow (offers, trades, withdrawals,
budget lock…). Where it used to sign with the stored key it now creates a
signing request and waits for the browser.

| Method | Path | Body → Result |
|---|---|---|
| GET | `/v1/wallet/signing/pending` | `{requests: [{id, envelope, summary, createdAt, expiresAt}]}` for the session user |
| POST | `/v1/wallet/signing/:id` | `{signature}` → `{ok, txId}`; the server verifies the signature against the envelope before use |
| POST | `/v1/wallet/signing/:id/reject` | `{reason?}` → `{ok}`; the waiting flow fails with `SIGNATURE_REJECTED` |

Also pushed as in-app notification `wallet.signing_request` `{id}` so the
site can react without polling; the site polls every 2 s as fallback while
a request is open. A request expires after 180 s → the flow fails with
`SIGNATURE_TIMEOUT`. `summary` is the server's human text (e.g. "Release
0.05 BTC of trade #123 to @bob") — the wallet ALSO decodes the action
itself and shows both.

### Session keys

| Method | Path | Body → Result |
|---|---|---|
| POST | `/v1/wallet/session` | `{scope: SessionScope[], ttlSecs}` → `{signingRequestId, key, expiresAt}`: server generates the session keypair (stored encrypted; scope-limited, cannot move funds) and queues an `AuthorizeSessionKey` signing request |
| GET | `/v1/wallet/session` | `{active: {key, scope, expiresAt} \| null}` |
| DELETE | `/v1/wallet/session` | queues `RevokeSessionKey` (or lets it expire) |

While a session is active and in scope, the server signs order-book
actions with it directly (no popup). Out-of-scope actions still go to the
wallet.

### Account mode

`GET /v1/chain/info` gains `keyMode: 'custodial' | 'wallet' | 'none'` and
`walletAddress`.

## 4. Chain: session keys and expiry

- `AuthorizeSessionKey { key: Address, scope: u32, expires_at: u64 }`
  signed by the principal (a session key cannot authorize another).
  `RevokeSessionKey { key }` by the principal or the session key itself.
  At most 16 live sessions per principal; `expires_at` ≤ 30 days ahead.
- Scope bits: `MARKETS = 1` (PlaceOrder, CancelOrder, HouseQuote);
  `P2P_MANAGE = 2` (CreateOffer, UpdateOffer, PauseOffer, CloseOffer,
  MarkPaid). Never in any scope: Transfer, Withdraw, ReleaseTrade,
  CancelTrade, StartTrade, LockBudget, UnlockBudget, Bond/Unbond/Delegate,
  governance, Attest, session management.
- Admission: the envelope's signer is the session key (own nonce, own
  account). If a live session maps it to a principal and the action is in
  scope, the action executes as the principal and spends the principal's
  budget; otherwise `Unauthorized`.
- Trade expiry: once `now > deadline` any address may `CancelTrade` an
  unpaid trade (buyer any time, as before). The marketplace's sweep signs
  with its own operator key; no user key needed.

## 5. Migration (custodial → wallet)

1. User links a wallet (`/v1/auth/wallet/link`). Precondition: no open
   offers, trades, orders, or unlock queue on the custodial address; the
   site lists what to close first.
2. Server, with the custodial key, signs one `Transfer` per asset of the
   full deposit balance to the wallet address, and the attester signs a
   fresh `Attest` with the current tier for the wallet address.
3. Server marks the account `wallet`: `chain_user_keys.address` = wallet
   address, `secret_enc = NULL`, `key_mode = 'wallet'`. From now on every
   signature is a signing request. The custodial secret is gone.
4. Every step is a chain tx id, shown in the Chain tab.

## 6. What the operator still signs (its own keys, never users')

Attester (KYC tiers), param admin (fees), treasury (awards, referral
payouts), house maker quotes, the trade-expiry sweep, session keys it is
authorized to hold. These stay in the marketplace's KMS/env as today.

## 7. Status (2026-09-09)

Implemented and verified on the local testnet:

- Chain: `AuthorizeSessionKey` / `RevokeSessionKey` (`keel-vm::modules::sessions`,
  scopes `MARKETS=1`, `P2P_MANAGE=2`, ≤16 live keys, ≤30 days, a session key
  has its own nonce and spends the principal's budget, may revoke itself
  after expiry); expired unpaid trades are cancelable by anyone. RPC
  account view: `sessions`, `session_of`. SDK: the two actions,
  `SESSION_SCOPE`, `signMessage` / `verifyMessage` / `messageDigest`.
- Extension `apps/wallet-extension` (MV3): BIP39 vault (PBKDF2 310k +
  AES-GCM), `window.keel` per §2, approval windows with the decoded action,
  per-origin-per-network connections, 43 tests; load `dist/` unpacked.
  Derivation: `secret_0 = BIP39 seed[0..32]`, `secret_i = sha256(seed ‖
  u32_le(i))`; vector `abandon×23 art` → `1de352e4…2961`.
- Marketplace: migration 0146 (`key_mode`, nullable secret,
  `chain_session_keys`); `signing-requests.ts` (in-memory queue, 180 s,
  signature verified against the stored envelope), `session-keys.ts`,
  `wallet-auth.ts` (challenges), `migration.ts` (balances moved,
  re-attested, deposit addresses retired, secret deleted); `ChainSigner`
  may be async; `ChainUserKeys.signerFor(user, action)` picks session key
  → wallet → custodial. Routes as in §3 plus `GET /v1/wallet/migration`
  and `GET /v1/moderation/users/:id/wallet`. Expired-trade sweeps for
  wallet-mode buyers are signed by the treasury key. Attestations are
  keyed by subject so a migrated address gets a fresh one.
- Web: "Sign in with Keel Wallet" on the login page, `SigningBridge`
  (polls pending requests, hands them to the extension, posts signatures
  or rejections, banner while waiting), Wallet → "Your keys" (mode,
  blockers, confirmation, migrate, trading-key on/off), chain-info
  `keyMode`.
- Backoffice: user page shows key mode, trading key, migration blockers.

Known gaps:

- Deposits sent to a retired custodial deposit address after migration are
  credited to the abandoned key. The site warns and retires the cached
  addresses; a chain action to reassign vault deposit indexes to the new
  owner (signed by the old key during migration) would close this.
- Sign-up with the wallet exists (`/v1/auth/wallet/register`, "Create
  account with Keel Wallet" on the register page) but still asks for an
  email: notifications, verification and KYC need it. There is no password;
  the reset flow can set one later.
- The marketplace holds session keys (scope-limited by the chain) and its
  own operator keys; nothing else.
