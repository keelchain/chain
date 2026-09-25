/** External block-explorer links for vault chains, per Keel network. */

export type ExternalChain = 'BTC' | 'ETH' | 'TRON';

const HOSTS: Record<string, Record<ExternalChain, string>> = {
  mainnet: { BTC: 'https://mempool.space', ETH: 'https://etherscan.io', TRON: 'https://tronscan.org/#' },
  testnet: { BTC: 'https://mempool.space/testnet4', ETH: 'https://sepolia.etherscan.io', TRON: 'https://nile.tronscan.org/#' },
};

function host(chain: string, network: string): string | null {
  const table = HOSTS[network === 'mainnet' ? 'mainnet' : 'testnet']!;
  return table[chain as ExternalChain] ?? null;
}

export function externalTxUrl(chain: string, txHash: string, network: string): string | null {
  const h = host(chain, network);
  if (!h) return null;
  const hash = chain === 'ETH' && !txHash.startsWith('0x') ? `0x${txHash}` : chain !== 'ETH' ? txHash.replace(/^0x/, '') : txHash;
  switch (chain) {
    case 'BTC':
      return `${h}/tx/${hash}`;
    case 'ETH':
      return `${h}/tx/${hash}`;
    case 'TRON':
      return `${h}/transaction/${hash}`;
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
      return `${h}/address/${address}`;
    case 'TRON':
      return `${h}/address/${address}`;
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
    default:
      return chain;
  }
}
