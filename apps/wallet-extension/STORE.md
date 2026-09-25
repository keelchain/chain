# Keel Wallet — store submission kit

Packages come from `npm run package` (→ `dist-store/`):

| File | Store |
|---|---|
| `keel-wallet-<v>-chromium.zip` | Chrome Web Store (also serves Brave and Opera users), Microsoft Edge Add-ons |
| `keel-wallet-<v>-firefox.zip` | Firefox Add-ons (AMO) |
| `keel-wallet-<v>-source.zip` | AMO "source code" upload: `apps/wallet-extension` plus `sdk/ts` (the bundled SDK) with `BUILD.md`; reviewers rebuild with Node 22 |

Bump `version` in `manifest.json` and `package.json` together before every upload; stores refuse a re-used version.

The Firefox add-on id (`browser_specific_settings.gecko.id`) must never be one
AMO has disabled before: an upload under a disabled id is disabled again
automatically, before review. Current id: `keel-wallet@keelchain.com` (1.1.1);
earlier ids are burned. Upload only from the mohab@keelchain.com developer
account.

## Listing

- **Name:** Keel Wallet
- **Summary (132 chars max):** The browser wallet for Keelchain, a settlement chain for peer-to-peer trading (public testnet). Your keys stay on your device.
- **Category:** Productivity → Developer tools (Chrome: "Productivity"; AMO: "Other"; Edge: "Productivity")
- **Language:** English
- **Homepage:** https://keelchain.com/wallet/
- **Support email:** wallet@keelchain.com — **developer account:** mohab@keelchain.com
- **Privacy policy URL:** https://keelchain.com/wallet/privacy.html
- **Description:**

  Keel Wallet is the browser wallet of Keelchain (keelchain.com), a settlement chain for peer-to-peer trading with on-chain escrow, vaults for bridged assets and a USD stablecoin.

  Keelchain is currently a public testnet. Nothing on it has monetary value and the wallet cannot hold or move real funds. It exists so people can try key-based sign-in and action signing on Keelchain sites before the chain launches.

  What it does: create a wallet from a 24-word phrase or import one, sign in to a Keelchain site with it instead of a password, and approve each action the site asks for — posting an offer, funding a trade, locking KEEL for capacity — from a prompt that shows exactly what is being signed. Keys are generated and stored only in this extension, encrypted with your password. Nothing is sent to any server except the signed actions you approve, and no data is collected.

  The extension activates on keelchain.com and on the sites of Keelchain clients (the first is a peer-to-peer marketplace). It exposes the `window.keel` provider those sites use.

- **Single purpose (Chrome):** provide the Keelchain key wallet to Keelchain sites.

## Permissions justification (paste into the store forms)

| Permission | Why |
|---|---|
| `storage` | Encrypted key material, settings and per-site connection approvals live in extension storage. |
| Content scripts on `keelchain.com`, `safethetrade.com`, `mohabmetwally.com`, `localhost` | Injects the `window.keel` provider only on Keelchain's own sites and its listed client sites, so those sites can ask the wallet to connect. Nothing runs until the site calls it, and every request needs the user's approval in the popup. No other site sees the extension. |
| Hosts, the same list | To read the chain RPC behind those sites and to notify open tabs of those sites when the account or network changes. |

No remote code, no analytics, no data collection. The privacy policy states this.

## Per-store checklist

**Chrome Web Store** (developer account, one-time fee): upload the chromium zip, fill the single-purpose and permission justifications above, add the privacy policy URL, mark "does not collect user data", add 1280×800 screenshots (create wallet, connect prompt, sign prompt) and the 128 px icon from `public/icons/128.png`. Brave and Opera users install from here.

**Microsoft Edge Add-ons**: same zip and texts; Partner Center account, no fee.

**Firefox Add-ons (AMO)**: upload the firefox zip as a listed add-on, then the source zip when asked; the add-on id is `wallet@keelchain.com` (`browser_specific_settings.gecko.id`, keep it forever; `wallet@keelchain.com` is the disabled 2026-09-15 listing). Minimum Firefox 140 (the data-collection declaration needs it). `npm run package` then `web-ext lint` on the unpacked firefox package should report no errors.

**Safari**: needs an Xcode conversion (`xcrun safari-web-extension-converter`) and an Apple developer account; not part of this kit.

## Screenshots and the end-to-end proof

`store-assets/01-create-wallet.png` … `04-your-keys.png` are 1280×800 store
screenshots: the sandbox site with the extension's own prompt composited at
the right (seed phrase, connect prompt, sign-in prompt, "Your keys" on the
wallet page). They were captured on 2026-09-15 by
`scripts/e2e-signup.mjs`, a Playwright script that loads `dist/` into
Chromium, creates a wallet in the popup, registers on
a client sandbox's register page with "Create account with Keel
wallet", approves the connect and sign prompts and lands on the wallet
page showing the chain address. Re-run it after any change to the
provider or the approval flow (`npm i playwright` in a scratch dir,
`node scripts/e2e-signup.mjs`; Playwright's own Chromium, not Google
Chrome ≥ 137, which dropped `--load-extension`). Each run creates a
throwaway `wallet-e2e-<time>@example.invalid` account on the sandbox.

## Before the first upload

- The only networks offered are the Keel testnet (chain id 3) and a local devnet; no mainnet entry exists until launch.
- The pages `site/wallet/index.html` and `site/wallet/privacy.html` in this repo must be live on keelchain.com BEFORE submitting, or the reviewer finds dead links.
- Bump `version` in `manifest.json` and `package.json` together for every upload.
