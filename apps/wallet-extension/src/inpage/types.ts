/**
 * Public types of the `window.keel` provider (docs/wallet.md §2).
 * This file is self-contained on purpose: a web app can copy it verbatim.
 */

export type ErrorCode = 'USER_REJECTED' | 'LOCKED' | 'NO_ACCOUNT' | 'NOT_CONNECTED' | 'INVALID_REQUEST' | 'WRONG_NETWORK';

/** Every rejected promise from the provider carries this shape. */
export interface ProviderError {
  code: ErrorCode;
  message: string;
}

export type SessionScope = 'markets' | 'p2p_manage';

/** Scope bits as the chain encodes them (`AuthorizeSessionKey.scope: u32`). */
export const SESSION_SCOPE_BITS: Record<SessionScope, number> = { markets: 1, p2p_manage: 2 };

export interface AccountInfo {
  /** 32-byte Ed25519 public key, lowercase hex. */
  address: string;
  /** Wallet network id, e.g. "testnet". */
  network: string;
  chainId: number;
}

export type Amount = bigint | number | string;
export type Address = string | number[];
export type Hash32 = string | number[];
export type Side = 'buy' | 'sell';
export type OrderType = 'limit' | 'market';
export type Chain = 'Bitcoin' | 'Ethereum' | 'Tron';
export type Role = 'Validator' | 'Observer' | 'Arbitrator';
export type VoteChoice = 'Yes' | 'No' | 'Abstain' | 'Veto';

export interface OfferSpec {
  side: Side;
  asset: string;
  fiat_currency: string;
  payment_method: string;
  margin_bps: number;
  fixed_price?: Amount | null;
  min_amount: Amount;
  max_amount: Amount;
  payment_window_secs: number;
  country?: string | null;
  min_tier: number;
  terms: string;
  instructions_hash: Hash32;
}

/**
 * The chain's action enum in its JSON (externally tagged) form. This mirrors
 * `@keelchain/sdk`'s `Action` for the user-facing variants plus the session-key
 * variants; operator/observer/governance variants are accepted as
 * `{ [kind: string]: unknown }` and shown raw by the wallet.
 */
export type Action =
  | { Transfer: { to: Address; asset: string; amount: Amount; memo?: string | null } }
  | { PlaceOrder: { pair: string; side: Side; order_type: OrderType; price?: Amount | null; quantity?: Amount | null; quote_budget?: Amount | null; client_id?: Amount | null } }
  | { CancelOrder: { order_id: Amount } }
  | { BuyBudget: { actions: Amount } }
  | { CreateOffer: OfferSpec }
  | { UpdateOffer: { offer_id: Amount; spec: OfferSpec } }
  | { PauseOffer: { offer_id: Amount; paused: boolean } }
  | { CloseOffer: { offer_id: Amount } }
  | { StartTrade: { offer_id: Amount; amount: Amount; fiat_amount: Amount; instructions_hash: Hash32 } }
  | { MarkPaid: { trade_id: Amount; proof_hash?: Hash32 | null } }
  | { ReleaseTrade: { trade_id: Amount } }
  | { CancelTrade: { trade_id: Amount } }
  | { OpenDispute: { trade_id: Amount; evidence_hash: Hash32 } }
  | { SubmitEvidence: { trade_id: Amount; evidence_hash: Hash32 } }
  | { RequestDepositAddress: { chain: Chain } }
  | { Withdraw: { asset: string; to: string; amount: Amount } }
  | { MintStable: { asset: string; amount: Amount } }
  | { BurnStable: { asset: string; amount: Amount } }
  | { Bond: { role: Role; amount: Amount; consensus_key?: Hash32 | null } }
  | { Unbond: { role: Role; amount: Amount } }
  | { Delegate: { validator: Address; amount: Amount } }
  | { Undelegate: { validator: Address; amount: Amount } }
  | 'ClaimRewards'
  | { Vote: { proposal_id: Amount; choice: VoteChoice } }
  | { ExecuteProposal: { proposal_id: Amount } }
  | { LockBudget: { amount: Amount } }
  | { UnlockBudget: { amount: Amount } }
  | { AuthorizeSessionKey: { key: Address; scope: number; expires_at: number } }
  | { RevokeSessionKey: { key: Address } }
  | { [kind: string]: Record<string, unknown> };

export interface Envelope {
  /** Must equal the connected address (hex). */
  signer: string;
  nonce: number;
  chain_id: number;
  action: Action;
}

/** The SDK's `SignedAction`: what `POST /v1/actions` accepts. */
export interface SignedAction {
  envelope: { signer: number[]; nonce: number; chain_id: number; action: Action };
  signature: string;
}

export interface SignActionRequest {
  envelope: Envelope;
  /** Optional caller context shown above the decoded action. */
  context?: { title?: string; description?: string };
}

export interface SignActionResult {
  signature: string;
  tx_id: string;
  signed: SignedAction;
}

export interface AuthorizeSessionRequest {
  /** The session public key (hex) that will act for the connected account. */
  key: string;
  scope: SessionScope[];
  /** Unix seconds; at most 30 days ahead. */
  expires_at: number;
  nonce: number;
  chain_id: number;
}

export type ProviderEventName = 'accountChanged' | 'disconnect' | 'networkChanged';

export interface KeelProvider {
  readonly isKeel: true;
  readonly version: string;

  /** Ask the user to connect this origin. Remembered per origin (per network) until disconnect. */
  connect(opts?: { network?: string }): Promise<AccountInfo>;
  disconnect(): Promise<void>;
  /** The connected account for this origin, or null (never prompts). */
  getAccount(): Promise<AccountInfo | null>;

  /** Sign an arbitrary message (login challenge). The popup shows the text. */
  signMessage(message: string): Promise<{ address: string; signature: string }>;

  /** Sign a chain action; the popup shows a decoded summary before the user approves. */
  signAction(req: SignActionRequest): Promise<SignActionResult>;

  /** Sugar for `signAction` with `AuthorizeSessionKey`; the popup explains scope and expiry. */
  authorizeSession(req: AuthorizeSessionRequest): Promise<SignActionResult>;

  on(event: ProviderEventName, handler: (payload: unknown) => void): () => void;
}

declare global {
  interface Window {
    keel?: KeelProvider;
  }
}
