import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// Dev server on :5178. Harbor has no backend of its own: it reads the node
// RPC and the indexer of the network chosen at runtime from VITE_NETWORKS
// (see README). Both serve CORS-enabled JSON, so nothing is proxied.
declare const process: { env: Record<string, string | undefined> };

export default defineConfig({
  base: process.env['VITE_BASE'] ?? '/',
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: 5178,
    strictPort: true,
  },
  preview: {
    port: 5178,
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    setupFiles: ['src/test-setup.ts'],
    css: false,
  },
});
