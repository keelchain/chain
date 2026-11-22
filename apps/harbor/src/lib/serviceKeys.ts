/**
 * Service keys: a second key a client authorizes to sign for its account
 * (`AuthorizeSessionKey`), with a nonce of its own, so the service that
 * runs day-to-day operations never needs the operator key.
 */

/** Scope bits a client grants to its own service. */
export const SCOPE_OPS = 4;
export const SCOPE_PAYOUTS = 8;

export const SCOPE_TEXT: { bit: number; label: string; detail: string }[] = [
  { bit: 1, label: 'Order book', detail: 'place and cancel orders' },
  { bit: 2, label: 'P2P offers', detail: 'manage offers, mark trades paid' },
  { bit: SCOPE_OPS, label: 'Operations', detail: 'attest users, issue their deposit addresses, move, swap and release balances inside your custody' },
  { bit: SCOPE_PAYOUTS, label: 'Payouts', detail: 'send withdrawals out of your vault from the accounts in your custody' },
];

/** The scopes in `bits`, as labels. */
export function scopeLabels(bits: number): string[] {
  return SCOPE_TEXT.filter((s) => (bits & s.bit) !== 0).map((s) => s.label);
}

/** A Keel address: 64 hex characters, with or without `0x`. Returns it lowercased, or null. */
export function parseKey(text: string): string | null {
  const t = text.trim().toLowerCase().replace(/^0x/, '');
  return /^[0-9a-f]{64}$/.test(t) ? t : null;
}

/**
 * A request carried in a link (`…/business/keys?key=<address>&scope=12&name=Bridge`),
 * so the service's operator sends a link and nobody copies an address.
 * Scope is limited to the two service permissions; anything else is dropped.
 */
export function requestFromQuery(search: string): { key: string; scope: number; name: string | null } | null {
  const q = new URLSearchParams(search);
  const key = parseKey(q.get('key') ?? '');
  if (!key) return null;
  const asked = Number(q.get('scope') ?? SCOPE_OPS);
  const scope = (Number.isInteger(asked) ? asked : SCOPE_OPS) & (SCOPE_OPS | SCOPE_PAYOUTS);
  const name = (q.get('name') ?? '').replace(/[^\w .-]/g, '').slice(0, 40).trim();
  return { key, scope: scope === 0 ? SCOPE_OPS : scope, name: name || null };
}

/** The link that asks an operator to authorize `key`. */
export function requestLink(base: string, key: string, scope: number, name?: string): string {
  const q = new URLSearchParams({ key, scope: String(scope) });
  if (name) q.set('name', name);
  return `${base}?${q.toString()}`;
}

/** Bytes as lowercase hex, the form the CLI and the SDK take a secret in. */
export function hexOf(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** Why this authorization cannot be sent, or null when it can. */
export function authorizeProblem(key: string | null, scope: number, days: number, own: string, maxDays: number): string | null {
  if (!key) return 'Paste the service\'s Keel address: 64 hex characters.';
  if (key === own) return 'That is this account\'s own address. A service key is a different key.';
  if (scope === 0) return 'Choose at least one permission.';
  if (!Number.isInteger(days) || days < 1 || days > maxDays) return `Choose 1 to ${maxDays} days.`;
  return null;
}
