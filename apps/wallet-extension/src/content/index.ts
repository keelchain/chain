/**
 * Content script (isolated world). Bridges the page's `window.keel` provider
 * (main world) to the background service worker. The origin the background
 * trusts comes from `sender`, never from the page.
 */
import { CHANNEL, isInpageToContent, isRuntimeMessage, type ContentToInpage, type ProviderResponse, type RuntimeProviderMessage } from '../core/protocol';

function toPage(msg: ContentToInpage): void {
  window.postMessage(msg, '*');
}

window.addEventListener('message', (ev: MessageEvent<unknown>) => {
  if (ev.source !== window || !isInpageToContent(ev.data)) return;
  const { request } = ev.data;
  const msg: RuntimeProviderMessage = { kind: 'keel:provider', request };
  const fail = (message: string): ProviderResponse => ({ id: request.id, ok: false, error: { code: 'INVALID_REQUEST', message } });
  let p: Promise<unknown>;
  try {
    p = chrome.runtime.sendMessage(msg);
  } catch (e) {
    toPage({ channel: CHANNEL, dir: 'to-page', response: fail(`Keel Wallet unavailable: ${e instanceof Error ? e.message : String(e)}`) });
    return;
  }
  p.then(
    (response) => {
      const r = response as ProviderResponse | undefined;
      toPage({ channel: CHANNEL, dir: 'to-page', response: r && r.id === request.id ? r : fail('Keel Wallet returned no response') });
    },
    (e: unknown) => toPage({ channel: CHANNEL, dir: 'to-page', response: fail(`Keel Wallet unavailable: ${e instanceof Error ? e.message : String(e)}`) }),
  );
});

chrome.runtime.onMessage.addListener((message: unknown) => {
  if (isRuntimeMessage(message) && message.kind === 'keel:event') toPage({ channel: CHANNEL, dir: 'to-page', event: message.event });
});
