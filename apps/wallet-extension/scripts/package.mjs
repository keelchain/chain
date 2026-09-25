// Store packages from dist/: one zip per browser family plus the source
// archive reviewers ask for (Firefox AMO requires it for bundled code).
//   dist-store/keel-wallet-<version>-chromium.zip   Chrome Web Store, Edge Add-ons, Brave, Opera
//   dist-store/keel-wallet-<version>-firefox.zip    Firefox AMO (background.scripts, gecko id)
//   dist-store/keel-wallet-<version>-source.zip     src/, popup, manifest, scripts, package files
// Run `npm run build` first; `npm run package` does both.
import { readFile, writeFile, rm, mkdir, readdir, stat, cp } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dist = path.join(root, 'dist');
const out = path.join(root, 'dist-store');
const manifest = JSON.parse(await readFile(path.join(dist, 'manifest.json'), 'utf8'));
const version = manifest.version;

await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

async function stage(name, mutate) {
  const dir = path.join(out, name);
  await cp(dist, dir, { recursive: true });
  const m = structuredClone(manifest);
  mutate(m);
  await writeFile(path.join(dir, 'manifest.json'), JSON.stringify(m, null, 2) + '\n');
  const zip = path.join(out, `keel-wallet-${version}-${name}.zip`);
  execFileSync('zip', ['-qr', '-X', zip, '.'], { cwd: dir });
  await rm(dir, { recursive: true, force: true });
  return zip;
}

// Chromium family: a service worker only; Chrome warns on the Firefox keys.
const chromium = await stage('chromium', (m) => {
  m.background = { service_worker: 'background.js' };
  delete m.browser_specific_settings;
});
// Firefox: MV3 event page via background.scripts; keep the gecko id and min version.
const firefox = await stage('firefox', (m) => {
  m.background = { scripts: ['background.js'] };
});
// Source archive for reviewers: everything needed to reproduce dist/.
const srcDir = path.join(out, 'source');
// Keep the repo layout (apps/wallet-extension + sdk/ts) so the
// file:../../sdk/ts link in package.json resolves for reviewers.
const extDir = path.join(srcDir, 'apps', 'wallet-extension');
const sdkSrc = path.resolve(root, '..', '..', 'sdk', 'ts');
const sdkDir = path.join(srcDir, 'sdk', 'ts');
await mkdir(extDir, { recursive: true });
await mkdir(sdkDir, { recursive: true });
for (const f of ['src', 'public', 'scripts', 'popup.html', 'manifest.json', 'package.json', 'package-lock.json', 'tsconfig.json', 'README.md', 'STORE.md', 'BUILD.md']) {
  try {
    await stat(path.join(root, f));
    await cp(path.join(root, f), path.join(extDir, f), { recursive: true });
  } catch {
    /* optional file */
  }
}
for (const f of ['src', 'test', 'package.json', 'package-lock.json', 'tsconfig.json', 'README.md']) {
  try {
    await stat(path.join(sdkSrc, f));
    await cp(path.join(sdkSrc, f), path.join(sdkDir, f), { recursive: true });
  } catch {
    /* optional file */
  }
}
await cp(path.join(root, 'BUILD.md'), path.join(srcDir, 'BUILD.md'));
const source = path.join(out, `keel-wallet-${version}-source.zip`);
execFileSync('zip', ['-qr', '-X', source, '.'], { cwd: srcDir });
await rm(srcDir, { recursive: true, force: true });
for (const f of await readdir(out)) console.log(`${f}  ${(await stat(path.join(out, f))).size} bytes`);
