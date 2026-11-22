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

/** Why this authorization cannot be sent, or null when it can. */
export function authorizeProblem(key: string | null, scope: number, days: number, own: string, maxDays: number): string | null {
  if (!key) return 'Paste the service\'s Keel address: 64 hex characters.';
  if (key === own) return 'That is this account\'s own address. A service key is a different key.';
  if (scope === 0) return 'Choose at least one permission.';
  if (!Number.isInteger(days) || days < 1 || days > maxDays) return `Choose 1 to ${maxDays} days.`;
  return null;
}
