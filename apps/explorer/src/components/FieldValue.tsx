import { Link } from 'react-router-dom';
import type { Field } from '../lib/decode';
import { formatBps, formatDuration, formatExact, formatInt } from '../lib/format';
import { chainName } from '../lib/links';
import { useHref } from '../network/NetworkContext';
import { AddressLink, Amount, AssetLink, Badge, ExternalLink, Hex, JsonView, PairLink, Price, BlockLink } from './ui';

/** Renders one decoded field with the right link/format for its kind. */
export function FieldValue({ field }: { field: Field }) {
  const href = useHref();
  switch (field.k) {
    case 'address':
      return <AddressLink address={field.value} full />;
    case 'validator':
      return (
        <span className="row">
          <AddressLink address={field.value} full />
          <Link to={href('/validators')} className="small">
            validators
          </Link>
        </span>
      );
    case 'amount':
      return <Amount value={field.value} asset={field.asset} />;
    case 'asset':
      return <AssetLink asset={field.value} />;
    case 'pair':
      return <PairLink pair={field.value} />;
    case 'price':
      return <Price value={field.value} pair={field.pair} />;
    case 'offer':
      return <Link to={href(`/offers/${field.id}`)}>Offer #{field.id}</Link>;
    case 'trade':
      return <Link to={href(`/trades/${field.id}`)}>Trade #{field.id}</Link>;
    case 'order':
      return <Link to={href(`/orders/${field.id}`)}>Order #{field.id}</Link>;
    case 'proposal':
      return <Link to={href(`/governance/${field.id}`)}>Proposal #{field.id}</Link>;
    case 'outbound':
      return <Link to={href('/vaults?tab=outbounds')}>Outbound #{field.id}</Link>;
    case 'hash':
      return <Hex value={field.value} full />;
    case 'external':
      return <ExternalLink chain={field.chain} value={field.value} what={field.what} />;
    case 'chain':
      return (
        <Link to={href(`/vaults/${field.value}`)}>
          {chainName(field.value)} ({field.value})
        </Link>
      );
    case 'height':
      return <BlockLink height={field.value} />;
    case 'timestamp':
      return <span>{formatExact(field.value)}</span>;
    case 'duration':
      return <span>{formatDuration(field.seconds)}</span>;
    case 'bps':
      return (
        <span>
          {formatBps(field.value)} <span className="muted small">({formatInt(field.value)} bps)</span>
        </span>
      );
    case 'number':
      return <span className="amount">{typeof field.value === 'number' ? formatInt(field.value) : /^\d+$/.test(field.value) ? formatInt(field.value) : field.value}</span>;
    case 'bool':
      return <Badge tone={field.value ? 'good' : 'neutral'}>{field.value ? 'yes' : 'no'}</Badge>;
    case 'badge':
      return <Badge tone={field.tone ?? 'neutral'}>{field.value}</Badge>;
    case 'json':
      return <JsonView value={field.value} />;
    case 'text':
    default:
      return <span style={{ whiteSpace: 'pre-wrap' }}>{field.value}</span>;
  }
}
