import type { EventRecord } from '../api/types';
import { decodeEvent } from '../lib/decode';
import { FieldValue } from './FieldValue';
import { Empty } from './ui';

/** Events as a vertical timeline; each field renders with links. */
export function EventsTimeline({ events }: { events: EventRecord[] }) {
  if (!events.length) return <Empty>No events were emitted.</Empty>;
  return (
    <ul className="timeline">
      {events.map((e, i) => {
        const d = decodeEvent(e);
        return (
          <li key={i} className={d.tone}>
            <div className="ev-head">
              <strong>{d.label}</strong>
              <span className="muted small mono">{d.type}</span>
            </div>
            <div className="ev-fields">
              {d.fields.map((f) => (
                <span key={f.name}>
                  <span className="name">{f.name}</span>
                  <FieldValue field={f.field} />
                </span>
              ))}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
