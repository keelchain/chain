/**
 * A client's public profile, checked the way the chain checks it
 * (`SetClientProfile`), so the form says what is wrong before the wallet
 * opens.
 */
export const NAME_MIN = 3;
export const NAME_MAX = 32;
export const WEBSITE_MAX = 100;
export const LOGO_MAX = 200;

/** A website as typed (`https://Acme.example/`) as the host name the chain takes (`acme.example`). */
export function hostOf(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/^https?:\/\//, '')
    .replace(/\/+$/, '');
}

export function nameProblem(name: string): string | null {
  if (name.length < NAME_MIN || name.length > NAME_MAX) return `${NAME_MIN} to ${NAME_MAX} characters.`;
  if (!/^[A-Za-z0-9 .-]+$/.test(name)) return 'Letters, digits, spaces, dots and hyphens only.';
  if (!/^[A-Za-z0-9]/.test(name) || !/[A-Za-z0-9]$/.test(name)) return 'Start and end with a letter or a digit.';
  if (name.includes('  ')) return 'One space at a time.';
  if (name.replace(/[^A-Za-z0-9]/g, '').length < NAME_MIN) return `At least ${NAME_MIN} letters or digits.`;
  return null;
}

/** `host` is what `hostOf` returned; empty means no website. */
export function websiteProblem(host: string): string | null {
  if (host === '') return null;
  const labels = host.split('.');
  const label = (l: string) => /^[a-z0-9-]{1,63}$/.test(l) && !l.startsWith('-') && !l.endsWith('-');
  if (host.length > WEBSITE_MAX || labels.length < 2 || !labels.every(label) || !/[a-z]/.test(labels[labels.length - 1]!)) {
    return 'A host name such as exchange.example, without a path or a port.';
  }
  return null;
}

export function logoProblem(logo: string): string | null {
  if (logo === '') return null;
  if (!logo.startsWith('https://') || logo.length <= 'https://'.length) return 'An https:// link to an image.';
  if (logo.length > LOGO_MAX) return `At most ${LOGO_MAX} characters.`;
  if (!/^[\x21-\x7e]+$/.test(logo) || /["'<>\\]/.test(logo)) return 'No spaces, quotes or angle brackets.';
  return null;
}

/** Where the proof of a website is served, and what the file holds. */
export function proofUrl(host: string): string {
  return `https://${host}/.well-known/keel.txt`;
}
