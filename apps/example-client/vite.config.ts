import { defineConfig } from 'vite';

declare const process: { env: Record<string, string | undefined> };

// Built by the site deploy into keelchain.com/wallet/try/.
export default defineConfig({
  base: process.env['VITE_BASE'] ?? '/',
  server: { host: '127.0.0.1', port: 5178, strictPort: true },
  build: { target: 'es2022' },
});
