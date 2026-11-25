import { afterEach, describe, expect, it, vi } from 'vitest';
import { noBackgroundMessage } from './ui';

const RAW = 'Could not establish connection. Receiving end does not exist.';
function manifest(background: object) {
  vi.stubGlobal('chrome', { runtime: { getManifest: () => ({ background }) } });
}
afterEach(() => vi.unstubAllGlobals());

describe('noBackgroundMessage', () => {
  it('names the Firefox package loaded in a Chromium browser', () => {
    manifest({ scripts: ['background.js'] });
    expect(noBackgroundMessage(new Error(RAW), false)).toMatch(/This is the Firefox package.*download the Chromium package/);
  });
  it('names the Chromium package loaded in Firefox', () => {
    manifest({ service_worker: 'background.js' });
    expect(noBackgroundMessage(new Error(RAW), true)).toMatch(/This is the Chromium package.*download the Firefox package/);
  });
  it('gives general advice when the package matches, and passes other errors through', () => {
    manifest({ service_worker: 'background.js' });
    expect(noBackgroundMessage(new Error(RAW), false)).toMatch(/Reload the extension/);
    expect(noBackgroundMessage(new Error('Unlock the wallet first.'), false)).toBe('Unlock the wallet first.');
  });
});
