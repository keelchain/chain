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
    if (message.method === 'listSites' || message.method === 'enableSite' || message.method === 'disableSite') {
      handleSites(message.method, message.params).then(
        (result) => sendResponse({ ok: true, result: toWire(result) }),
        (e: unknown) => sendResponse({ ok: false, error: e instanceof Error ? e.message : String(e) }),
      );
      return true;
    }
    wallet.handleUi(message.method, message.params).then(
      (result) => sendResponse({ ok: true, result: toWire(result) }),
      (e: unknown) => sendResponse({ ok: false, error: e instanceof UiError || e instanceof Error ? e.message : String(e) }),
    );
    return true;
  }
  return false;
});

// ------------------------------------------------------------ sites
//
// The provider is injected on Keelchain's own sites and localhost by the
// manifest. Any other site is enabled by the user from the popup: the popup
// asks the browser for that origin (optional host permission, needs the
// click), then the background registers the content scripts for it. The
// registration persists across restarts; the list below is what the popup
// shows and what a reinstall re-registers.

const SITES_KEY = 'enabledSites';

async function enabledSites(): Promise<string[]> {
  const got = await chrome.storage.local.get(SITES_KEY);
  const list = got[SITES_KEY];
  return Array.isArray(list) ? list.filter((x): x is string => typeof x === 'string') : [];
}

function scriptIds(origin: string): [string, string] {
  const tag = origin.replace(/[^a-z0-9]/gi, '_');
  return [`keel-content-${tag}`, `keel-inpage-${tag}`];
}

async function registerSite(origin: string): Promise<void> {
  const [contentId, inpageId] = scriptIds(origin);
  const matches = [`${origin}/*`];
  const existing = await chrome.scripting.getRegisteredContentScripts({ ids: [contentId, inpageId] });
  if (existing.length > 0) await chrome.scripting.unregisterContentScripts({ ids: existing.map((x) => x.id) });
  await chrome.scripting.registerContentScripts([
    { id: contentId, matches, js: ['content.js'], runAt: 'document_start', allFrames: false, persistAcrossSessions: true },
    { id: inpageId, matches, js: ['inpage.js'], runAt: 'document_start', allFrames: false, persistAcrossSessions: true, world: 'MAIN' },
  ]);
}

async function handleSites(method: string, params: unknown): Promise<unknown> {
  const p = (typeof params === 'object' && params !== null ? params : {}) as Record<string, unknown>;
  const origin = typeof p['origin'] === 'string' ? p['origin'] : '';
  if (method === 'listSites') return { sites: await enabledSites() };
  let u: URL;
  try {
    u = new URL(origin);
  } catch {
    throw new Error('Not a site origin.');
  }
  if (u.origin !== origin || !['https:', 'http:'].includes(u.protocol)) throw new Error('Not a site origin.');
  const sites = await enabledSites();
  if (method === 'enableSite') {
    const granted = await chrome.permissions.contains({ origins: [`${origin}/*`] });
    if (!granted) throw new Error('The browser did not grant access to that site.');
    await registerSite(origin);
    if (!sites.includes(origin)) sites.push(origin);
    await chrome.storage.local.set({ [SITES_KEY]: sites });
    return { sites };
  }
  const [contentId, inpageId] = scriptIds(origin);
  await chrome.scripting.unregisterContentScripts({ ids: [contentId, inpageId] }).catch(() => undefined);
  await chrome.permissions.remove({ origins: [`${origin}/*`] }).catch(() => undefined);
  const left = sites.filter((s) => s !== origin);
  await chrome.storage.local.set({ [SITES_KEY]: left });
  return { sites: left };
}

// After an update or reinstall the registrations are gone: put them back
// for every site the user enabled and the browser still allows.
chrome.runtime.onInstalled.addListener(() => {
  enabledSites().then(async (sites) => {
    for (const origin of sites) {
      const ok = await chrome.permissions.contains({ origins: [`${origin}/*`] }).catch(() => false);
      if (ok) await registerSite(origin).catch(() => undefined);
    }
  }, () => undefined);
});
