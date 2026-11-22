/** Networks as the chain's actions name them, and what a payout of an asset may be paid in. */
export const CHAINS = ['Bitcoin', 'Ethereum', 'Tron', 'Bsc', 'Solana'] as const;
export type Chain = (typeof CHAINS)[number];
export const CHAIN_NAME: Record<Chain, string> = { Bitcoin: 'Bitcoin', Ethereum: 'Ethereum', Tron: 'Tron', Bsc: 'BNB Smart Chain', Solana: 'Solana' };
export const CHAIN_BY_CODE: Record<string, Chain> = { BTC: 'Bitcoin', ETH: 'Ethereum', TRON: 'Tron', BSC: 'Bsc', SOL: 'Solana' };

/** The chain as the action names it, from the code (`BTC`) or the name the RPC returns. */
export function chainOf(v: string): Chain | null {
  return CHAIN_BY_CODE[v] ?? (CHAINS.includes(v as Chain) ? (v as Chain) : null);
}

/**
 * The networks a payout of `asset` can be paid on: for the unified dollar
 * balance, each network's own coin; `null` for an asset that lives on one
 * network and leaves on it.
 */
export function payoutRails(asset: string, stable: string): string[] | null {
  return asset === stable ? [`TRON.${stable}`, `BSC.${stable}`, `ETH.${stable}`] : null;
}
