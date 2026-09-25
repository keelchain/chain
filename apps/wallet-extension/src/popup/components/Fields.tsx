import type { NamedField } from '../../core/decode';
import { truncateHex } from '../../core/format';

function jsonWithBigints(v: unknown): string {
  return JSON.stringify(v, (_k, x: unknown) => (typeof x === 'bigint' ? x.toString() : x), 2);
}

export function FieldValue({ f }: { f: NamedField['field'] }) {
  switch (f.k) {
    case 'address':
    case 'hash':
      return <code title={f.value}>{truncateHex(f.value, 10, 8)}</code>;
    case 'amount':
    case 'price':
      return <span className="strong" title={`${f.value} smallest units`}>{f.formatted}</span>;
    case 'asset':
    case 'pair':
    case 'text':
      return <span>{f.value}</span>;
    case 'id':
      return <span>{f.what} #{f.id}</span>;
    case 'external':
      return <code title={f.value}>{f.value}</code>;
    case 'number':
      return <span>{f.value}</span>;
    case 'bool':
      return <span>{f.value ? 'yes' : 'no'}</span>;
    case 'timestamp':
      return <span title={String(f.value)}>{f.formatted}</span>;
    case 'duration':
      return <span>{f.formatted}</span>;
    case 'badge':
      return <span className={`badge ${f.tone}`}>{f.value}</span>;
    case 'json':
      return <pre className="json">{jsonWithBigints(f.value)}</pre>;
  }
}

export function Fields({ fields }: { fields: NamedField[] }) {
  if (fields.length === 0) return null;
  return (
    <dl className="fields">
      {fields.map((nf, i) => (
        <div key={i} className="field">
          <dt>{nf.name}</dt>
          <dd><FieldValue f={nf.field} /></dd>
        </div>
      ))}
    </dl>
  );
}

export function RawJson({ value }: { value: unknown }) {
  return <pre className="json">{jsonWithBigints(value)}</pre>;
}
