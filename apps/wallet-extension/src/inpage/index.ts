// Entry of the script that runs in the page's main world (manifest content_scripts, world: MAIN).
import { createProvider, windowTransport } from './provider';

(() => {
  if (typeof window === 'undefined') return;
  if (window.keel?.isKeel) return;
  const provider = createProvider(windowTransport(window));
  Object.defineProperty(window, 'keel', { value: provider, writable: false, configurable: false, enumerable: true });
  window.dispatchEvent(new Event('keel#initialized'));
})();
