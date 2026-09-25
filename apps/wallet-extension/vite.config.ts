import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

// `vite` (dev) serves the popup only; the real extension bundle comes from
// scripts/build.mjs, which runs four Vite builds (popup, background, content,
// inpage) so that the non-popup entries are single classic scripts.
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: 'dist',
    emptyOutDir: false,
    rollupOptions: { input: { popup: 'popup.html' } },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
