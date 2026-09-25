import { useState } from 'react';
import { truncateHex } from '../../core/format';

export function Address({ value, full = false }: { value: string; full?: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    navigator.clipboard.writeText(value).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    }, () => undefined);
  };
  return (
    <span className="address">
      <code title={value}>{full ? value : truncateHex(value, 10, 8)}</code>
      <button className="mini" onClick={copy}>{copied ? 'Copied' : 'Copy'}</button>
    </span>
  );
}
