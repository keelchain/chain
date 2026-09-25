import type { ExplorerApi, SearchKind } from '../api/types';

export interface SearchTarget {
  kind: SearchKind;
  ref: string;
}

/** Path for a search result within a network base (`/testnet`). */
export function pathFor(kind: SearchKind, ref: string): string {
  switch (kind) {
    case 'block':
      return `/blocks/${ref}`;
    case 'tx':
      return `/tx/${ref}`;
    case 'account':
      return `/account/${ref}`;
    case 'validator':
      return `/account/${ref}`;
    case 'asset':
      return `/assets/${encodeURIComponent(ref)}`;
    case 'market':
      return `/markets/${encodeURIComponent(ref)}`;
    case 'offer':
      return `/offers/${ref}`;
    case 'trade':
      return `/trades/${ref}`;
  }
}

/** Purely syntactic classification, used before asking the indexer. */
export function classifyLocally(q: string): SearchTarget | null {
  const s = q.trim();
  if (!s) return null;
  if (/^\d+$/.test(s)) return { kind: 'block', ref: s };
  const m = /^(offer|trade|block|tx)\s*#?\s*([0-9a-fx]+)$/i.exec(s);
  if (m) {
    const kind = m[1]!.toLowerCase() as SearchKind;
    return { kind, ref: m[2]!.replace(/^0x/i, '') };
  }
  if (/^[A-Za-z0-9.]+[/-][A-Za-z0-9.]+$/.test(s)) return { kind: 'market', ref: s.toUpperCase() };
  return null;
}

/** Resolves a query to a path (relative to the network base) or null. */
export async function resolveSearch(api: ExplorerApi, q: string): Promise<{ path: string } | { suggestions: SearchTarget[] } | null> {
  const s = q.trim();
  if (!s) return null;
  try {
    const r = await api.search(s);
    if (r.kind && r.ref) return { path: pathFor(r.kind, r.ref) };
    if (r.suggestions?.length) return { suggestions: r.suggestions.map((x) => ({ kind: x.kind, ref: x.ref })) };
  } catch {
    // indexer unavailable: fall back to syntax
  }
  const local = classifyLocally(s);
  if (local) return { path: pathFor(local.kind, local.ref) };
  const hex = s.replace(/^0x/i, '').toLowerCase();
  if (/^[0-9a-f]{64}$/.test(hex)) return { path: pathFor('account', hex) };
  return { suggestions: [] };
}
