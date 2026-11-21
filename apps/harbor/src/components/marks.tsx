/** Harbor's own marks, drawn inline so they take the colour of the text around them. */

/** The harbour mark: an anchor. In the header beside the name, and faint behind every page. */
export function HarborMark({ size = 22, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden focusable={false}>
      <circle cx="12" cy="4.600" r="2.100" />
      <path d="M12 6.700V21" />
      <path d="M8.200 10.400h7.600" />
      <path d="M4 13.500c0 4.300 3.600 7.500 8 7.500s8-3.200 8-7.500" />
      <path d="M2.600 15.300 4 13.500l1.900 1.300M18.100 14.800 20 13.500l1.400 1.800" />
    </svg>
  );
}

/** The orca, for KEEL amounts: the coin's smallest unit is named after it. */
export function OrcaMark({ size = 20, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={(size * 20) / 32} viewBox="0 0 32 20" aria-hidden focusable={false}>
      <path fill="currentColor" d="M2 11.500C2.500 7.500 8 5.200 14 5.800c5 .5 9 2.700 12 4.200l4.500-3.500c.3 2-.5 3.500-2.200 4.700 1.700.8 2.700 2.100 2.700 4L26 12.800c-5 3.200-14 4.200-20 2.700-2.500-.7-4.200-2-4-4Z" />
      <path fill="currentColor" d="M13.500 5.900c.3-2.900 1.300-4.600 3-5.400-.3 2 .3 4 1.700 5.800Z" />
      <path fill="currentColor" d="M9.500 15.800c.5 1.700 2 3 4 3.400-.5-1.400-.5-2.400.1-3.300Z" />
      <path fill="var(--bg, #fff)" d="M3.200 13.200c3.800 2.600 10.800 2.800 16.800 1-6 .4-12 .1-16.800-1Z" />
      <ellipse fill="var(--bg, #fff)" cx="7.800" cy="9.200" rx="2" ry="0.900" transform="rotate(-15 7.800 9.200)" />
    </svg>
  );
}
