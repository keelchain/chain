/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_NETWORKS?: string;
  readonly VITE_API_MOCK?: string;
  readonly VITE_BASE?: string;
  readonly VITE_EXPLORER_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
