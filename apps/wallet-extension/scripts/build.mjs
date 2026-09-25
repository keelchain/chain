// Builds dist/: popup (ES modules), background/content/inpage (single IIFE
// scripts), manifest.json and generated icons. Run via `npm run build`.
import { build } from 'vite';
import react from '@vitejs/plugin-react';
import { rm, mkdir, copyFile, writeFile } from 'node:fs/promises';
import { deflateSync } from 'node:zlib';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const dist = path.join(root, 'dist');

await rm(dist, { recursive: true, force: true });
await mkdir(path.join(dist, 'icons'), { recursive: true });

// 1. popup: a normal Vite app build.
await build({
  root,
  configFile: false,
  logLevel: 'warn',
  plugins: [react()],
  build: { outDir: dist, emptyOutDir: false, rollupOptions: { input: { popup: path.join(root, 'popup.html') } } },
});

// 2. background, content, inpage: one self-contained classic script each.
for (const [name, entry] of [
  ['background', 'src/background/index.ts'],
  ['content', 'src/content/index.ts'],
  ['inpage', 'src/inpage/index.ts'],
]) {
  await build({
    root,
    configFile: false,
    logLevel: 'warn',
    build: {
      outDir: dist,
      emptyOutDir: false,
      lib: { entry: path.join(root, entry), name: `keel_${name}`, formats: ['iife'], fileName: () => `${name}.js` },
      rollupOptions: { output: { extend: true, inlineDynamicImports: true } },
      minify: false,
      sourcemap: false,
    },
  });
}

// 3. manifest + icons.
await copyFile(path.join(root, 'manifest.json'), path.join(dist, 'manifest.json'));
// Icons, favicon and logo mark are the Keel brand assets in public/
// (rasterized from apps/web/public/favicon.svg), copied as-is.
for (const size of [16, 32, 48, 128]) await copyFile(path.join(root, 'public', 'icons', `${size}.png`), path.join(dist, 'icons', `${size}.png`));
for (const f of ['favicon.svg', 'logo-mark.svg']) await copyFile(path.join(root, 'public', f), path.join(dist, f));
console.log(`built ${path.relative(process.cwd(), dist)}`);

// Minimal PNG encoder: a rounded blue square with a white "S" bar motif.

function crc32(buf) {
  let c = ~0;
  for (const b of buf) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}
