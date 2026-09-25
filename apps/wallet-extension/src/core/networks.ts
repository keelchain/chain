export interface Network {
  id: string;
  name: string;
  rpc: string;
  chainId: number;
  explorer: string;
  /** Placeholder networks are listed but cannot be used for signing yet. */
  placeholder?: boolean;
}

export const NETWORKS: readonly Network[] = [
  // The public sandbox chain (2026-09-15): node RPC and explorer behind
  // testnet.keelchain.com, chain id 3.
  { id: 'testnet', name: 'Keel testnet', rpc: 'https://testnet.keelchain.com/rpc', chainId: 3, explorer: 'https://testnet.keelchain.com/testnet' },
  // A developer's local devnet (infra/dev/devnet.sh, chain id 1).
  { id: 'devnet', name: 'Local devnet', rpc: 'http://127.0.0.1:5000', chainId: 1, explorer: 'http://127.0.0.1:5177/testnet' },
];

export const DEFAULT_NETWORK_ID = 'testnet';

export function findNetwork(id: string): Network | undefined {
  return NETWORKS.find((n) => n.id === id);
}

export function explorerAccountUrl(n: Network, address: string): string {
  return n.explorer ? `${n.explorer.replace(/\/$/, '')}/account/${address}` : '';
}

export function explorerTxUrl(n: Network, txId: string): string {
  return n.explorer ? `${n.explorer.replace(/\/$/, '')}/tx/${txId}` : '';
}
