/** The external chains the popup shows, and their network names per Keel network. */
import { externalNetworks, type ExternalChain } from '@keelchain/sdk';

export const DEPOSIT_CHAINS: ExternalChain[] = ['BTC', 'TRON', 'BSC', 'SOL'];

/** The bridged assets a user can hold in Keel, by chain. */
export const BRIDGED_ASSETS: { asset: string; chain: ExternalChain }[] = [
  { asset: 'BTC.BTC', chain: 'BTC' },
  { asset: 'TRON.USDT', chain: 'TRON' },
  { asset: 'TRON.TRX', chain: 'TRON' },
  { asset: 'BSC.USDT', chain: 'BSC' },
  { asset: 'BSC.BNB', chain: 'BSC' },
  { asset: 'SOL.USDC', chain: 'SOL' },
  { asset: 'SOL.SOL', chain: 'SOL' },
];

/** Name of the external network a Keel network bridges: the testnet bridges signet and Nile. */
export function externalName(networkId: string, chain: ExternalChain): string {
  return externalNetworks(networkId === 'mainnet' ? 'mainnet' : 'testnet')[chain].name;
}

/** Amount text ("1.25") to smallest units, or null when it is not a valid amount for `decimals`. */
export function toUnits(text: string, decimals: number): string | null {
  const [whole, frac = ''] = text.trim().split('.');
  if (!/^\d+$/.test(whole ?? '') || !/^\d*$/.test(frac) || frac.length > decimals) return null;
  return (BigInt(whole ?? '0') * 10n ** BigInt(decimals) + BigInt((frac + '0'.repeat(decimals)).slice(0, decimals))).toString();
}
