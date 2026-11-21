/** The mark of an external network, drawn inline so the popup loads no image. */
import type { ExternalChain } from '@keelchain/sdk';

/** The ticker a network is known by, and the coins the chain credits from it. */
export const CHAIN_TICKER: Record<ExternalChain, { ticker: string; coins: string }> = {
  BTC: { ticker: 'BTC', coins: 'Bitcoin' },
  TRON: { ticker: 'TRX', coins: 'USDT · TRX' },
  BSC: { ticker: 'BNB', coins: 'USDT · BNB' },
  SOL: { ticker: 'SOL', coins: 'USDC · SOL' },
  ETH: { ticker: 'ETH', coins: 'USDT · ETH' },
};

export function ChainLogo({ chain, size = 24 }: { chain: ExternalChain; size?: number }) {
  const common = { width: size, height: size, viewBox: '0 0 32 32', 'aria-hidden': true, focusable: false } as const;
  switch (chain) {
    case 'BTC':
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="16" fill="#f7931a" />
          <path
            fill="#fff"
            d="M22.5 14.1c.3-2-1.2-3.1-3.300-3.800l.7-2.700-1.700-.4-.7 2.600-1.300-.3.700-2.700-1.700-.4-.7 2.700-3.300-.8-.4 1.800s1.200.3 1.200.3c.7.2.8.6.8 1l-1.900 7.500c-.1.200-.3.500-.8.400 0 0-1.200-.3-1.200-.3l-.8 1.900 3.200.8-.700 2.800 1.700.4.700-2.700 1.300.3-.700 2.700 1.700.4.700-2.800c2.800.5 5 .3 5.900-2.200.7-2.100 0-3.200-1.500-4 1.100-.3 1.900-1 2.100-2.500zm-3.700 5.200c-.5 2.100-4 1-5.100.7l.900-3.700c1.100.3 4.800.8 4.200 3zm.5-5.300c-.5 1.900-3.400.9-4.300.7l.8-3.300c.9.2 4 .7 3.500 2.600z"
          />
        </svg>
      );
    case 'TRON':
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="16" fill="#ef0027" />
          <path fill="#fff" d="M22.900 10.800 8 8l7.800 19.700 10.900-13.300-3.800-3.600zm-.2 1.200 2.300 2.200-6.200 1.100 3.900-3.300zm-5.300 3-6.500-5.400 10.700 2-4.200 3.400zm-.5 1 -1.100 8.800L10 10.300l6.900 5.700zm1 .4 6.900-1.200-7.900 9.600 1-8.400z" />
        </svg>
      );
    case 'BSC':
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="16" fill="#f3ba2f" />
          <path fill="#fff" d="m12.100 14.400 3.900-3.900 3.900 3.900 2.300-2.300L16 6l-6.200 6.100 2.300 2.300zM6 16l2.300-2.300 2.300 2.300-2.300 2.300L6 16zm6.100 1.600 3.900 3.900 3.900-3.900 2.300 2.300L16 26l-6.200-6.100 2.300-2.300zm9.300-1.600 2.300-2.300L26 16l-2.300 2.300-2.300-2.300zm-3.100 0L16 13.700 13.700 16l2.300 2.300 2.300-2.300z" />
        </svg>
      );
    case 'SOL':
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="16" fill="#14141f" />
          <path fill="#14f195" d="M10.700 19.500h12.100l-2.500 2.500H8.200l2.500-2.500z" />
          <path fill="#7c8cf8" d="M8.200 14.700h12.100l2.500 2.500H10.700l-2.500-2.500z" />
          <path fill="#c46cf6" d="M10.700 10h12.100l-2.500 2.500H8.200l2.500-2.500z" />
        </svg>
      );
    default:
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="16" fill="#627eea" />
          <path fill="#fff" d="M16 5 9.500 16.200 16 20l6.500-3.800L16 5zm0 16.300-6.500-3.800L16 27l6.500-9.500-6.500 3.800z" />
        </svg>
      );
  }
}
