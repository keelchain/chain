import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// Dev server on :5177. The explorer talks to one indexer per network, chosen
// at runtime from VITE_NETWORKS (see README); nothing is proxied because the
// indexer serves CORS-enabled JSON and a WebSocket at /v1/ws.
declare const process: { env: Record<string, string | undefined> };

export default defineConfig({
  base: process.env['VITE_BASE'] ?? '/',
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 5177,
    strictPort: true,
  },
  preview: {
    port: 5177,
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    setupFiles: ['src/test-setup.ts'],
    css: false,
  },
});
