// Refresh site/wallet/shots from the built extension: welcome, seed, home (funded by the
// testnet faucet), receive, connect and sign. Playwright's own Chromium is required:
// Google Chrome 137+ ignores --load-extension. Usage: node scripts/shots.mjs <out-dir>
// (cd to a dir with playwright installed, e.g. npx -p playwright node scripts/shots.mjs out).
import { writeFileSync } from 'node:fs';
import http from 'node:http';
const EXT = new URL('../dist', import.meta.url).pathname;
const OUT = process.argv[2];
const HEADED = process.env.HEADED === '1';
const log = (...a) => console.log('[shots]', ...a);
// A local site the extension injects into (manifest allows http://localhost/*).
const SITE_PORT = 8765;
const SANDBOX_HTML = `<!doctype html><title>Keel sandbox</title><body style="font:16px system-ui;padding:24px">
  <h1>Keel sandbox</h1><p id="s">waiting for wallet…</p>
  <script>
    window.go = async (what) => {
      const k = window.keel;
      if (what === 'connect') return k.connect({ network: 'testnet' });
      if (what === 'sign') return k.signMessage('Sign in to Keelchain sandbox\\n' + new Date().toISOString());
    };
    window.addEventListener('keel#initialized', () => { document.getElementById('s').textContent = 'wallet detected'; });
    if (window.keel) document.getElementById('s').textContent = 'wallet detected';
  </script></body>`;
const site = http.createServer((req, res) => { res.setHeader('content-type', 'text/html'); res.end(SANDBOX_HTML); });
await new Promise((r) => site.listen(SITE_PORT, '127.0.0.1', r));
const ctx = await chromium.launchPersistentContext(`/tmp/keel-wallet-shots-${Date.now()}`, {
  headless: false,
  viewport: { width: 380, height: 640 }, deviceScaleFactor: 2,
  args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, '--no-first-run', ...(HEADED ? [] : ['--headless=new'])],
});
let [sw] = ctx.serviceWorkers();
if (!sw) sw = await ctx.waitForEvent('serviceworker', { timeout: 20000 });
const extId = new URL(sw.url()).host;
log('extension', extId);
const shot = async (page, name, w, h) => { await page.setViewportSize({ width: w, height: h }); await page.waitForTimeout(600); await page.screenshot({ path: `${OUT}/${name}.png` }); log('saved', name, `${w*2}x${h*2}`); };
const popup = await ctx.newPage();
await popup.goto(`chrome-extension://${extId}/popup.html`);
await popup.waitForTimeout(800);
await shot(popup, 'welcome', 380, 440);
await popup.getByRole('button', { name: 'Create a new wallet' }).click();
await popup.waitForSelector('ol.words li');
await shot(popup, 'seed', 380, 600);
const words = await popup.locator('ol.words li').allTextContents();
writeFileSync(`${OUT}/phrase.txt`, words.join(' ') + '\n', { mode: 0o600 });
await popup.getByLabel('I wrote the phrase down').check();
await popup.locator('input[type=password]').nth(0).fill('shots-pass-123');
await popup.locator('input[type=password]').nth(1).fill('shots-pass-123');
await popup.getByRole('button', { name: 'Create wallet' }).click();
await popup.waitForTimeout(1500);
const text = await popup.locator('body').innerText();
const addr = (text.match(/[0-9a-f]{64}/) || [])[0];
log('home text:', text.replace(/\s+/g, ' ').slice(0, 200)); log('address', addr);
// Fund it from the testnet faucet so the home card shows balances.
if (addr) {
  const r = await fetch('https://testnet.keelchain.com/api/v1/faucet', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ address: addr }) });
  log('faucet', r.status, (await r.text()).slice(0, 160));
  await popup.waitForTimeout(12000);
  await popup.reload(); await popup.waitForTimeout(2500);
}
await popup.evaluate(() => window.scrollTo(0, 0)); await popup.waitForTimeout(300);
await shot(popup, 'home', 380, 640);
// Receive card.
const btn = popup.getByRole('button', { name: 'Bitcoin signet' }).first();
if (await btn.count()) {
  await btn.click(); await popup.waitForTimeout(3000);
  const card = popup.locator('text=Receive').first();
  await card.scrollIntoViewIfNeeded().catch(() => {});
  await popup.evaluate(() => { const el = [...document.querySelectorAll('.label')].find((e) => e.textContent.trim() === 'Receive'); el?.closest('section,div.card,div')?.scrollIntoView({ block: 'start' }); });
  await shot(popup, 'receive', 380, 640);
} else log('no Bitcoin signet button');
// Connect + sign prompts from the sandbox site.
const page = await ctx.newPage();
await ctx.route('https://sandbox.keelchain.com/**', (route) => route.fulfill({ status: 200, contentType: 'text/html', body: SANDBOX_HTML }));
await page.goto('https://sandbox.keelchain.com/', { waitUntil: 'load' });
await page.waitForTimeout(800);
log('provider on site:', await page.evaluate(() => typeof window.keel));
const approval = async (name, h) => {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = ctx.pages().find((x) => x.url().includes('popup.html#approve'));
    if (p) {
      await p.waitForSelector('button:has-text("Approve")', { timeout: 15000 });
      await shot(p, name, 380, h);
      await p.getByRole('button', { name: 'Approve' }).click({ noWaitAfter: true, timeout: 5000 }).catch((e) => { if (!p.isClosed()) throw e; });
      return;
    }
    await page.waitForTimeout(250);
  }
  throw new Error(`no approval window for ${name}`);
};
const c = page.evaluate(() => window.go('connect'));
await approval('connect', 490);
log('connect ->', JSON.stringify(await c).slice(0, 120));
const s = page.evaluate(() => window.go('sign'));
await approval('sign', 490);
log('sign ->', JSON.stringify(await s).slice(0, 80));
await ctx.close(); site.close();
log('done');
