/** Service worker entry: wires the Wallet controller to chrome.* APIs. */
import { isRuntimeMessage, toProviderError, toWire, type ProviderEvent, type ProviderResponse } from '../core/protocol';
import { ChromeLocalStore } from '../core/storage';
import { Approvals } from './approvals';
import { Session } from './session';
import { UiError, Wallet } from './wallet';

const session = new Session();
const approvals = new Approvals();

function emit(origin: string | null, event: ProviderEvent): void {
  const msg = { kind: 'keel:event' as const, event: { event: event.event, payload: toWire(event.payload) } };
  chrome.tabs.query({}).then((tabs) => {
    for (const tab of tabs) {
      if (tab.id === undefined || !tab.url) continue;
      let tabOrigin: string;
      try {
        tabOrigin = new URL(tab.url).origin;
      } catch {
        continue;
      }
      if (origin !== null && tabOrigin !== origin) continue;
      chrome.tabs.sendMessage(tab.id, msg).catch(() => undefined);
    }
  }, () => undefined);
}

const wallet = new Wallet({ store: new ChromeLocalStore(), session, approvals, emit });

// ------------------------------------------------------------ approval window

let approvalWindowId: number | null = null;
let opening: Promise<void> | null = null;

async function openApprovalWindow(): Promise<void> {
  if (approvalWindowId !== null) {
    try {
      await chrome.windows.update(approvalWindowId, { focused: true });
      return;
    } catch {
      approvalWindowId = null;
    }
  }
  if (opening) return opening;
  opening = (async () => {
    const url = chrome.runtime.getURL('popup.html#approve');
    const win = await chrome.windows.create({ url, type: 'popup', width: 400, height: 660, focused: true });
    approvalWindowId = win?.id ?? null;
  })().finally(() => {
    opening = null;
  });
  return opening;
}

approvals.onAdded = () => {
  openApprovalWindow().catch(() => undefined);
};
approvals.onEmpty = () => {
  if (approvalWindowId !== null) {
    const id = approvalWindowId;
    approvalWindowId = null;
    chrome.windows.remove(id).catch(() => undefined);
  }
};
chrome.windows.onRemoved.addListener((id) => {
  if (id !== approvalWindowId) return;
  approvalWindowId = null;
  // Closing the window without deciding: rejected, or still locked.
  approvals.rejectAll(session.unlocked ? 'USER_REJECTED' : 'LOCKED');
});

// ------------------------------------------------------------ messages

function senderOrigin(sender: chrome.runtime.MessageSender): string | null {
  if (typeof sender.origin === 'string' && sender.origin !== 'null') return sender.origin;
  if (sender.url) {
    try {
      return new URL(sender.url).origin;
    } catch {
      return null;
    }
  }
  return null;
}

function isExtensionPage(sender: chrome.runtime.MessageSender): boolean {
  return sender.id === chrome.runtime.id && typeof sender.url === 'string' && sender.url.startsWith(chrome.runtime.getURL(''));
}

chrome.runtime.onMessage.addListener((message: unknown, sender, sendResponse) => {
  if (!isRuntimeMessage(message)) return false;
  if (message.kind === 'keel:provider') {
    const origin = sender.tab ? senderOrigin(sender) : null;
    const { request } = message;
    const respond = (r: ProviderResponse) => sendResponse(r);
    if (!origin) {
      respond({ id: request.id, ok: false, error: { code: 'INVALID_REQUEST', message: 'Requests must come from a page' } });
      return false;
    }
    wallet.handleProvider(origin, request).then(
      (result) => respond({ id: request.id, ok: true, result }),
      (e: unknown) => respond({ id: request.id, ok: false, error: toProviderError(e) }),
    );
    return true;
  }
  if (message.kind === 'keel:ui') {
    if (!isExtensionPage(sender)) {
      sendResponse({ ok: false, error: 'forbidden' });
      return false;
    }
    wallet.handleUi(message.method, message.params).then(
      (result) => sendResponse({ ok: true, result: toWire(result) }),
      (e: unknown) => sendResponse({ ok: false, error: e instanceof UiError || e instanceof Error ? e.message : String(e) }),
    );
    return true;
  }
  return false;
});
