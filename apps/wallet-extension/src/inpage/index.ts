// Entry of the script that runs in the page's main world (manifest content_scripts, world: MAIN).
import { createProvider, windowTransport } from './provider';

(() => {
  if (typeof window === 'undefined') return;
  if (window.keel?.isKeel) return;
  const provider = createProvider(windowTransport(window));
  // `window.keel` is the provider; `window.stt` stays as an alias for
  // clients written against the earlier name (SafeTheTrade's web app).
  for (const name of ['keel', 'stt'] as const) {
    Object.defineProperty(window, name, { value: provider, writable: false, configurable: false, enumerable: true });
  }
  window.dispatchEvent(new Event('keel#initialized'));
  window.dispatchEvent(new Event('stt#initialized'));
})();
