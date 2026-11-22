import { describe, expect, it } from 'vitest';
import { authorizeProblem, parseKey, scopeLabels } from './serviceKeys';

const A = 'ab'.repeat(32);
const ME = 'cd'.repeat(32);

describe('service keys', () => {
  it('reads an address with or without 0x, in any case', () => {
    expect(parseKey(`0x${A.toUpperCase()}`)).toBe(A);
    expect(parseKey(` ${A} `)).toBe(A);
    expect(parseKey('abc')).toBeNull();
    expect(parseKey('zz'.repeat(32))).toBeNull();
  });
  it('names the scopes in a bitset', () => {
    expect(scopeLabels(12)).toEqual(['Operations', 'Payouts']);
    expect(scopeLabels(4)).toEqual(['Operations']);
    expect(scopeLabels(0)).toEqual([]);
  });
  it('refuses a missing key, the account itself, no scope, and a bad expiry', () => {
    expect(authorizeProblem(null, 12, 29, ME, 29)).toMatch(/64 hex/);
    expect(authorizeProblem(ME, 12, 29, ME, 29)).toMatch(/own address/);
    expect(authorizeProblem(A, 0, 29, ME, 29)).toMatch(/at least one/);
    expect(authorizeProblem(A, 12, 30, ME, 29)).toMatch(/1 to 29/);
    expect(authorizeProblem(A, 12, 29, ME, 29)).toBeNull();
  });
});
