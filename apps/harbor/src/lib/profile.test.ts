import { describe, expect, it } from 'vitest';
import { hostOf, logoProblem, nameProblem, proofUrl, websiteProblem } from './profile';

describe('client profile', () => {
  it('accepts the names the chain accepts', () => {
    for (const ok of ['Acme', 'Acme Exchange', 'acme-x', 'Acme 2.0', 'A1b']) expect(nameProblem(ok), ok).toBeNull();
    for (const bad of ['ab', 'a'.repeat(33), ' Acme', 'Acme-', 'Acme  Exchange', 'Acme <b>', 'A.-.']) expect(nameProblem(bad), bad).not.toBeNull();
  });

  it('turns what is typed into a host name and checks it', () => {
    expect(hostOf(' https://Acme.Example/ ')).toBe('acme.example');
    for (const ok of ['', 'acme.example', 'trade.acme-x.example']) expect(websiteProblem(ok), ok).toBeNull();
    for (const bad of ['localhost', '10.0.0.1', 'acme.example/path', 'acme.example:8443', '-acme.example', 'acme..example']) {
      expect(websiteProblem(bad), bad).not.toBeNull();
    }
    expect(proofUrl('acme.example')).toBe('https://acme.example/.well-known/keel.txt');
  });

  it('takes only an https link as a logo', () => {
    for (const ok of ['', 'https://acme.example/logo.png']) expect(logoProblem(ok), ok).toBeNull();
    for (const bad of ['http://acme.example/logo.png', 'https://', 'https://acme.example/a b.png', 'https://acme.example/a"x', `https://a.example/${'a'.repeat(200)}`]) {
      expect(logoProblem(bad), bad).not.toBeNull();
    }
  });
});
