import { describe, expect, it } from 'vitest';
import { authorizeProblem, parseKey, requestFromQuery, requestLink, scopeLabels } from './serviceKeys';

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
  it('reads a request from a link and keeps only the service permissions', () => {
    expect(requestFromQuery(`?key=${A}&scope=12&name=Bridge`)).toEqual({ key: A, scope: 12, name: 'Bridge' });
    expect(requestFromQuery(`?key=${A}`)).toEqual({ key: A, scope: 4, name: null });
    expect(requestFromQuery(`?key=${A}&scope=15&name=<b>x</b>`)).toEqual({ key: A, scope: 12, name: 'bxb' });
    expect(requestFromQuery('?key=nope')).toBeNull();
    expect(requestFromQuery(new URL(requestLink('https://h/testnet/business/keys', A, 12, 'Bridge')).search)).toEqual({ key: A, scope: 12, name: 'Bridge' });
  });
  it('refuses a missing key, the account itself, no scope, and a bad expiry', () => {
    expect(authorizeProblem(null, 12, 29, ME, 29)).toMatch(/64 hex/);
    expect(authorizeProblem(ME, 12, 29, ME, 29)).toMatch(/own address/);
    expect(authorizeProblem(A, 0, 29, ME, 29)).toMatch(/at least one/);
    expect(authorizeProblem(A, 12, 30, ME, 29)).toMatch(/1 to 29/);
    expect(authorizeProblem(A, 12, 29, ME, 29)).toBeNull();
  });
});
