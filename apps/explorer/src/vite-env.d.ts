/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_NETWORKS?: string;
  readonly VITE_API_MOCK?: string;
  readonly VITE_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
