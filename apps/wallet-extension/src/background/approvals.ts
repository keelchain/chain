/** Pending user approvals: created by provider requests, decided by the popup. */
import type { DecodedAction } from '../core/decode';
import type { ErrorCode, SessionScope } from '../inpage/types';
import { WalletError } from '../core/protocol';

export interface NetworkRef {
  id: string;
  name: string;
  chainId: number;
}

interface Base {
  id: string;
  origin: string;
  address: string;
  network: NetworkRef;
  createdAt: number;
}

export type ApprovalRequest =
  | (Base & { kind: 'connect' })
  | (Base & { kind: 'signMessage'; message: string })
  | (Base & { kind: 'signAction'; envelope: { signer: string; nonce: number; chain_id: number; action: unknown }; decoded: DecodedAction; context?: { title?: string; description?: string } })
  | (Base & {
      kind: 'authorizeSession';
      envelope: { signer: string; nonce: number; chain_id: number; action: unknown };
      decoded: DecodedAction;
      sentence: string;
      scope: SessionScope[];
      expires_at: number;
      key: string;
    });

interface Pending {
  request: ApprovalRequest;
  resolve: () => void;
  reject: (e: WalletError) => void;
}

export class Approvals {
  private readonly pending = new Map<string, Pending>();
  /** Called when a request is added (the background opens the popup). */
  onAdded: ((req: ApprovalRequest) => void) | null = null;
  onEmpty: (() => void) | null = null;

  list(): ApprovalRequest[] {
    return Array.from(this.pending.values()).map((p) => p.request).sort((a, b) => a.createdAt - b.createdAt);
  }

  get size(): number {
    return this.pending.size;
  }

  /** Resolves when approved, rejects with USER_REJECTED / LOCKED otherwise. */
  ask(request: ApprovalRequest): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this.pending.set(request.id, { request, resolve, reject });
      this.onAdded?.(request);
    });
  }

  decide(id: string, approved: boolean, code: ErrorCode = 'USER_REJECTED'): ApprovalRequest {
    const p = this.pending.get(id);
    if (!p) throw new WalletError('INVALID_REQUEST', 'No such pending request');
    this.pending.delete(id);
    if (approved) p.resolve();
    else p.reject(new WalletError(code));
    if (this.pending.size === 0) this.onEmpty?.();
    return p.request;
  }

  /** Rejects everything, e.g. when the approval window is closed. */
  rejectAll(code: ErrorCode): void {
    for (const id of Array.from(this.pending.keys())) this.decide(id, false, code);
  }
}
