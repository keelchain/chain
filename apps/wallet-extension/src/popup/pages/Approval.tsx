import { useState } from 'react';
import type { ApprovalRequest } from '../../background/approvals';
import { call, type UiState } from '../ui';
import { Fields, RawJson } from '../components/Fields';
import { Address } from '../components/Address';
import { formatDate } from '../../core/format';
import { scopeWords } from '../../core/decode';

export function Approval({ request, queued, onDecided }: { request: ApprovalRequest; queued: number; onDecided: (s: UiState) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showRaw, setShowRaw] = useState(false);

  const decide = async (approved: boolean) => {
    setBusy(true);
    try {
      onDecided(await call<UiState>('decideApproval', { id: request.id, approved }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const title = { connect: 'Connect to this site?', signMessage: 'Sign this message?', signAction: 'Approve this action?', authorizeSession: 'Authorize a session key?' }[request.kind];

  return (
    <div className="screen approval">
      <header>
        <h2>{title}</h2>
        <div className="origin">{request.origin}</div>
        {queued > 0 && <div className="muted small">{queued} more request{queued > 1 ? 's' : ''} waiting</div>}
      </header>

      {request.kind === 'connect' && (
        <div className="card">
          <p>The site will see your address and may ask you to sign messages and actions. Every signature still needs your approval.</p>
          <div className="label">Account</div>
          <Address value={request.address} />
          <div className="label">Network</div>
          <div>{request.network.name} (chain id {request.network.chainId})</div>
        </div>
      )}

      {request.kind === 'signMessage' && (
        <div className="card">
          <div className="label">Message</div>
          <pre className="message">{request.message}</pre>
          <p className="muted small">Signing a message never moves funds and cannot be replayed as a chain action.</p>
          <div className="label">Signing as</div>
          <Address value={request.address} />
        </div>
      )}

      {(request.kind === 'signAction' || request.kind === 'authorizeSession') && (
        <>
          {request.kind === 'authorizeSession' && (
            <div className="card highlight">
              <p className="strong">{request.sentence}</p>
              <div className="label">Allowed to</div>
              <div>{scopeWords(request.scope)}</div>
              <div className="label">Until</div>
              <div>{formatDate(request.expires_at)}</div>
              <div className="label">Session key</div>
              <Address value={request.key} />
            </div>
          )}
          {request.kind === 'signAction' && request.context && (request.context.title || request.context.description) && (
            <div className="card highlight">
              {request.context.title && <div className="strong">{request.context.title}</div>}
              {request.context.description && <div className="muted">{request.context.description}</div>}
            </div>
          )}
          <div className="card">
            <div className="row"><span className={`badge ${request.decoded.warning ? 'warn' : 'neutral'}`}>{request.decoded.label}</span></div>
            <p className="strong">{request.decoded.summary}</p>
            {request.decoded.warning && <p className="warning">{request.decoded.warning}</p>}
            <Fields fields={request.decoded.fields} />
          </div>
          <div className="card">
            <dl className="fields">
              <div className="field"><dt>Signer</dt><dd><Address value={request.envelope.signer} /></dd></div>
              <div className="field"><dt>Nonce</dt><dd>{request.envelope.nonce}</dd></div>
              <div className="field"><dt>Network</dt><dd>{request.network.name} (chain id {request.envelope.chain_id})</dd></div>
            </dl>
            <button className="ghost small" onClick={() => setShowRaw((v) => !v)}>{showRaw ? 'Hide raw action' : 'Show raw action'}</button>
            {showRaw && <RawJson value={request.envelope.action} />}
          </div>
        </>
      )}

      {error && <p className="error">{error}</p>}
      <footer className="actions">
        <button onClick={() => void decide(false)} disabled={busy}>Reject</button>
        <button className="primary" onClick={() => void decide(true)} disabled={busy}>Approve</button>
      </footer>
    </div>
  );
}
