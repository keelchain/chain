# Building Keel Wallet from source

Requires Node 22 (the same the store packages were built with).

```
cd sdk/ts && npm ci && npm run build          # @keelchain/sdk, linked by the extension as file:../../sdk/ts
cd ../../apps/wallet-extension && npm ci && npm run build   # -> dist/
node scripts/package.mjs                     # -> dist-store/ (chromium, firefox, source zips)
```

`dist/` is what the store zips contain: `popup.html` + `assets/` (the popup,
a Vite/React build), `background.js`, `content.js`, `inpage.js` (each a
single self-contained script bundled by Vite/Rollup, unminified),
`manifest.json` and the icons. No code is fetched at runtime.
