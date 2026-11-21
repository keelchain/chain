/**
 * Amount arithmetic. Everything on chain is an integer in smallest units,
 * so everything here is BigInt until the final display.
 */
import { formatAmount } from './format';

export const KEEL_DECIMALS = 6;
/** USD micro and the billing asset's smallest unit are the same thing. */
export const USD_DECIMALS = 6;

/** A chain amount (decimal string, number or bigint) as a BigInt; 0 when absent or malformed. */
export function big(v: string | number | bigint | null | undefined): bigint {
  if (typeof v === 'bigint') return v;
  if (typeof v === 'number') return Number.isFinite(v) ? BigInt(Math.trunc(v)) : 0n;
  if (typeof v === 'string' && /^-?\d+$/.test(v.trim())) return BigInt(v.trim());
  return 0n;
}

/** Decimal text to smallest units, or null when malformed or too precise. */
export function toUnits(text: string, decimals: number): bigint | null {
  const t = text.trim();
  if (!/^\d+(\.\d+)?$/.test(t)) return null;
  const [whole, frac = ''] = t.split('.');
  if (frac.length > decimals) return null;
  return BigInt(whole!) * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals) || '0');
}

/** Smallest units as plain decimal text for an input field ("1500.25"). */
export function fromUnits(units: bigint, decimals: number): string {
  return formatAmount(units, decimals, { maxFraction: decimals }).replace(/,/g, '');
}

/** `part / whole` as a number in [0, 1]; 0 when the whole is 0. */
export function ratio(part: bigint, whole: bigint): number {
  if (whole <= 0n || part <= 0n) return 0;
  if (part >= whole) return 1;
  return Number((part * 10_000n) / whole) / 10_000;
}

/** `a - b`, never below zero. */
export function shortfall(a: bigint, b: bigint): bigint {
  return a > b ? a - b : 0n;
}

/** Orcas as "100,000 KEEL". */
export function keel(orcas: bigint | string): string {
  return `${formatAmount(orcas, KEEL_DECIMALS)} KEEL`;
}

/** USD micro as "300 USDT" (the billing asset's symbol). */
export function money(micro: bigint | string, asset: string): string {
  return `${formatAmount(micro, USD_DECIMALS)} ${asset}`;
}

/** The spendable (`deposit`) balance of `asset`. */
export function spendable(balances: { asset: string; account_type: string; balance: string }[] | undefined, asset: string): bigint {
  return big(balances?.find((b) => b.asset === asset && b.account_type === 'deposit')?.balance);
}
