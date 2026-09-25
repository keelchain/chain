// Drives the real signup-with-wallet flow on the sandbox with the built
// extension loaded into Chrome, and captures screenshots for the stores.
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'node:fs';
mkdirSync(process.env['SHOTS_DIR'] ?? '/tmp/keel-wallet-e2e', { recursive: true });

const EXT = new URL('../dist', import.meta.url).pathname;
const SITE = process.env.E2E_SITE ?? 'https://mohabmetwally.com/stt'; // a client sandbox that offers wallet sign-up
const OUT = process.env['SHOTS_DIR'] ?? '/tmp/keel-wallet-e2e';
const email = `wallet-e2e-${Date.now()}@example.invalid`;
const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);

const ctx = await chromium.launchPersistentContext(`/tmp/keel-wallet-e2e-profile-${Date.now()}`, {
  headless: false, viewport: { width: 1280, height: 800 },
  args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, '--no-first-run'],
});
let [sw] = ctx.serviceWorkers();
if (!sw) sw = await ctx.waitForEvent('serviceworker', { timeout: 20000 });
const extId = new URL(sw.url()).host;
log('extension id', extId);

// 1. Onboarding in the popup page.
const popup = await ctx.newPage();
await popup.setViewportSize({ width: 400, height: 660 });
await popup.goto(`chrome-extension://${extId}/popup.html`);
await popup.getByRole('button', { name: 'Create a new wallet' }).click();
const words = await popup.locator('ol.words li').allTextContents();
if (words.length !== 24) throw new Error(`expected 24 words, got ${words.length}`);
writeFileSync(`${OUT}/test-wallet-phrase.txt`, words.join(' ') + '\n', { mode: 0o600 });
await popup.screenshot({ path: `${OUT}/popup-seed.png` });
await popup.getByLabel('I wrote the phrase down').check();
await popup.locator('input[type=password]').nth(0).fill('sandbox-pass-123');
await popup.locator('input[type=password]').nth(1).fill('sandbox-pass-123');
await popup.getByRole('button', { name: 'Create wallet' }).click();
await popup.waitForTimeout(1500);
await popup.screenshot({ path: `${OUT}/popup-home.png` });
const homeText = await popup.locator('body').innerText();
log('popup after create:', homeText.replace(/\s+/g, ' ').slice(0, 160));

// 2. Register on the site with the wallet.
const page = await ctx.newPage();
await page.goto(`${SITE}/register`, { waitUntil: 'networkidle' });
await page.screenshot({ path: `${OUT}/site-register.png` });
const hasProvider = await page.evaluate(() => typeof window.stt);
log('window.stt on the site:', hasProvider);
await page.getByPlaceholder('e.g. satoshi@protonmail.com').fill(email);
await page.locator('.auth-country-trigger').click();
await page.getByRole('combobox', { name: 'Search countries' }).fill('Egypt');
await page.locator('li[role=option]', { hasText: 'Egypt' }).first().click();
const approvals = [];
const approve = async (label) => {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = ctx.pages().find((x) => x.url().includes('popup.html#approve') && !approvals.includes(x));
    if (p) {
      await p.waitForSelector('button:has-text("Approve")', { timeout: 15000 });
      await p.setViewportSize({ width: 400, height: 660 }).catch(() => {});
      await p.screenshot({ path: `${OUT}/popup-approve-${label}.png` });
      const t = (await p.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 200);
      log(`approval (${label}):`, t);
      approvals.push(p);
      // The window closes itself once the decision is sent; a click that
      // races that close throws although the approval went through.
      await p.getByRole('button', { name: 'Approve' }).click({ noWaitAfter: true, timeout: 5000 }).catch((e) => { if (!p.isClosed()) throw e; });
      return;
    }
    await page.waitForTimeout(300);
  }
  throw new Error(`no approval window for ${label}`);
};
await page.getByRole('button', { name: 'Create account with Keel Wallet' }).click();
await approve('connect');
await approve('sign');
await page.waitForTimeout(4000);
log('url after signup:', page.url());
const bodyText = (await page.locator('body').innerText()).replace(/\s+/g, ' ');
log('page text:', bodyText.slice(0, 240));
await page.goto(`${SITE}/wallet`, { waitUntil: 'load' });
await page.waitForTimeout(6000);
await page.screenshot({ path: `${OUT}/site-wallet.png` });
const keys = page.locator('text=Your keys').first();
if (await keys.count()) { await keys.scrollIntoViewIfNeeded(); await page.waitForTimeout(800); await page.screenshot({ path: `${OUT}/site-wallet-keys.png` }); }
await page.goto(`${SITE}/account`, { waitUntil: 'load' }); await page.waitForTimeout(4000); await page.screenshot({ path: `${OUT}/site-account.png` });
const walletText = (await page.locator('body').innerText()).replace(/\s+/g, ' ');
log('wallet page:', walletText.slice(0, 300));
writeFileSync(`${OUT}/result.json`, JSON.stringify({ email, extId, url: page.url(), walletText: walletText.slice(0, 600) }, null, 2));
await ctx.close();
