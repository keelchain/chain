/** External block-explorer links for vault chains, per Keel network. */

export type ExternalChain = 'BTC' | 'ETH' | 'TRON' | 'BSC' | 'SOL';

const HOSTS: Record<string, Record<ExternalChain, string>> = {
  mainnet: { BTC: 'https://mempool.space', ETH: 'https://etherscan.io', TRON: 'https://tronscan.org/#', BSC: 'https://bscscan.com', SOL: 'https://solscan.io' },
  // The testnet bridges Bitcoin signet (not testnet3/4), Tron Nile and the BSC testnet.
  testnet: { BTC: 'https://mempool.space/signet', ETH: 'https://sepolia.etherscan.io', TRON: 'https://nile.tronscan.org/#', BSC: 'https://testnet.bscscan.com', SOL: 'https://solscan.io' },
};

function host(chain: string, network: string): string | null {
  const table = HOSTS[network === 'mainnet' ? 'mainnet' : 'testnet']!;
  return table[chain as ExternalChain] ?? null;
}

/** Solscan needs the cluster on the testnet (devnet). */
function cluster(chain: string, network: string): string {
  return chain === 'SOL' && network !== 'mainnet' ? '?cluster=devnet' : '';
}

export function externalTxUrl(chain: string, txHash: string, network: string): string | null {
  const h = host(chain, network);
  if (!h) return null;
  const evm = chain === 'ETH' || chain === 'BSC';
  const hash = evm && !txHash.startsWith('0x') ? `0x${txHash}` : !evm ? txHash.replace(/^0x/, '') : txHash;
  switch (chain) {
    case 'BTC':
      return `${h}/tx/${hash}`;
    case 'ETH':
    case 'BSC':
      return `${h}/tx/${hash}`;
    case 'TRON':
      return `${h}/transaction/${hash}`;
    case 'SOL':
      // `hash` is the signature when the caller passed the native id
      // (`external_tx`); the chain's own 32-byte hash is not linkable.
      return hash.length >= 86 ? `${h}/tx/${hash}${cluster(chain, network)}` : null;
    default:
      return null;
  }
}

export function externalAddressUrl(chain: string, address: string, network: string): string | null {
  const h = host(chain, network);
  if (!h) return null;
  switch (chain) {
    case 'BTC':
      return `${h}/address/${address}`;
    case 'ETH':
    case 'BSC':
      return `${h}/address/${address}`;
    case 'TRON':
      return `${h}/address/${address}`;
    case 'SOL':
      return `${h}/account/${address}${cluster(chain, network)}`;
    default:
      return null;
  }
}

export function chainName(chain: string): string {
  switch (chain) {
    case 'BTC':
    case 'Bitcoin':
      return 'Bitcoin';
    case 'ETH':
    case 'Ethereum':
      return 'Ethereum';
    case 'TRON':
    case 'Tron':
      return 'Tron';
    case 'BSC':
    case 'Bsc':
      return 'BNB Smart Chain';
    case 'SOL':
    case 'Solana':
      return 'Solana';
    default:
      return chain;
  }
}

export function chainCode(chain: string): string {
  switch (chain) {
    case 'Bitcoin':
      return 'BTC';
    case 'Ethereum':
      return 'ETH';
    case 'Tron':
      return 'TRON';
    case 'Bsc':
      return 'BSC';
    case 'Solana':
      return 'SOL';
    default:
      return chain;
  }
}
