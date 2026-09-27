# Connect Keel Wallet to your site or app

Keel Wallet is the user's key. A site never sees it: it asks the wallet to
connect, to sign a login challenge, to sign a chain action, or to authorize
a session key, and the user approves each request in the wallet's own
window. This page is the complete contract for a client that adds a
"Connect Keel Wallet" button, on the web or in a mobile app.

## 1. The provider

The extension injects `window.keel` on any site the user has enabled it
for (Keelchain's own sites and localhost are enabled out of the box; for
every other site the user clicks *Enable on this site* once in the wallet,
which asks the browser for that origin's permission). Nothing else about
the extension changes per site, so a client never waits for a wallet
release.

```ts
interface KeelProvider {
  readonly isKeel: true;
  readonly version: string;
  connect(opts?: { network?: string }): Promise<{ address, network, chainId }>;
  disconnect(): Promise<void>;
  getAccount(): Promise<{ address, network, chainId } | null>;   // never prompts
  signMessage(message: string): Promise<{ address, signature }>;
  signAction(req: { envelope: { signer, nonce, chain_id, action }, context? }): Promise<{ signature, tx_id, signed }>;
  authorizeSession(req: { key, scope: ("markets" | "p2p_manage")[], expires_at, nonce, chain_id }): Promise<{ signature, tx_id, signed }>;
  on(event: "accountChanged" | "disconnect" | "networkChanged", handler): () => void;
}
```

Rejections carry `{ code, message }` with codes `USER_REJECTED`, `LOCKED`,
`NO_ACCOUNT`, `NOT_CONNECTED`, `INVALID_REQUEST`, `WRONG_NETWORK`. The
provider fires `keel#initialized` on `window` when it is ready.

## 2. The kit in the SDK

`@keelchain/sdk` ships the browser and server halves:

```ts
import { detectProvider, connectWallet, mountConnectButton, loginChallenge,
         verifyLoginChallenge, requestSession, RpcClient } from "@keelchain/sdk";

// A button, no framework needed. In React, call connectWallet() from your own button.
mountConnectButton(document.getElementById("keel")!, {
  installUrl: "https://keelchain.com/wallet/",
  onConnected: async (account, provider) => {
    // 1. Login: the server issues a challenge, the wallet signs it, the server verifies.
    const { challenge } = await api.post("/auth/keel/challenge", { address: account.address });
    const { signature } = await provider.signMessage(challenge);
    await api.post("/auth/keel/login", { address: account.address, challenge, signature });
  },
});
```

Server side (Node):

```ts
import { loginChallenge, verifyLoginChallenge } from "@keelchain/sdk";

// issue
const challenge = loginChallenge(address, { site: "example.com" });   // "Keel login\naddress: …\nnonce: …\nissued: …\nsite: example.com"
store(nonceOf(challenge), address);
// verify (throws on a bad signature, a foreign address, an old challenge, an unknown nonce)
verifyLoginChallenge(address, challenge, signature, { maxAgeMs: 5 * 60_000, expectNonce, site: "example.com" });
```

The signed text carries the `keel-message-v1` domain, so a login signature
can never be replayed as a chain action.

## 3. Signing actions

For anything that moves the user's funds, the site builds the envelope and
asks the wallet to sign it; the wallet shows the decoded action and the
user approves:

```ts
const rpc = new RpcClient("https://testnet.keelchain.com/rpc");
const acct = await rpc.account(account.address);
const { signed, tx_id } = await provider.signAction({
  envelope: { signer: account.address, nonce: acct.nonce, chain_id: account.chainId,
              action: { Transfer: { to, asset: "KUSD", amount: 25_000_000n, memo: "invoice 42" } } },
  context: { title: "Pay invoice 42", description: "25 KUSD to the merchant" },
});
await rpc.submit(signed);
```

## 4. Session keys for trading

A popup per order is not usable. The site asks once for a session key with
a scope, keeps the secret server side for that user, and signs order-book
and offer actions with it until it expires (at most 30 days). A session key
cannot transfer, withdraw or release escrow; those still go to the wallet.

```ts
const { key, expiresAt } = await requestSession(provider, { scopes: ["markets"], nonce: acct.nonce, chainId: account.chainId });
// later, server side:
await rpc.send(key, { PlaceOrder: { pair: "BTC-KUSD", side: "buy", order_type: "limit", price, quantity } }, chainId);
```

## 5. Mobile apps

A browser extension cannot run inside a native or hybrid app. Two ways:

- **Embed the key model.** The wallet's core (BIP39 seed, key derivation,
  encrypted vault, action encoding and decoding) has no browser
  dependencies and is being split out as `@keelchain/wallet-core`, so a
  Capacitor, React Native or native app can hold the user's key with the
  same derivation the extension uses: the same seed phrase restores the
  same accounts anywhere.
- **Deep link to a mobile Keel Wallet.** `keel://connect?origin=…&challenge=…`
  is reserved for the standalone mobile wallet; until it ships, embedding
  is the way.

## 6. Networks

The wallet knows the public testnet and a local devnet, and the user can add
a custom network (RPC URL and chain id). `connect({ network })` asks for a
specific one; a mismatch rejects with `WRONG_NETWORK`.

## 7. Trying it

`keelchain.com/wallet/try` is a one-page client built from
`apps/example-client`: connect, read balances, sign a transfer, and watch
the account channel live. Its source is the shortest complete integration.
