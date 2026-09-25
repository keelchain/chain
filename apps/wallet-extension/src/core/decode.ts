/**
 * Decodes an action (the node's externally tagged JSON) into a labelled,
 * human-readable view for the approval screen. Same approach as
 * apps/explorer/src/lib/decode.ts, trimmed to what a wallet shows. Unknown
 * kinds fall back to a generic key/value listing.
 */
import { untag } from './actions';
import { decimalsOf, formatAmount, formatDate, formatDuration, sentence, splitPair } from './format';
import { scopesFromBits } from './actions';
import type { SessionScope } from '../inpage/types';

export type Field =
  | { k: 'address'; value: string }
  | { k: 'amount'; value: string; asset: string; formatted: string }
  | { k: 'price'; value: string; pair: string; formatted: string }
  | { k: 'asset'; value: string }
  | { k: 'pair'; value: string }
  | { k: 'id'; what: 'offer' | 'trade' | 'order' | 'proposal'; id: string }
  | { k: 'hash'; value: string }
  | { k: 'external'; chain: string; value: string }
  | { k: 'number'; value: string }
  | { k: 'bool'; value: boolean }
  | { k: 'text'; value: string }
  | { k: 'timestamp'; value: number; formatted: string }
  | { k: 'duration'; seconds: number; formatted: string }
  | { k: 'badge'; value: string; tone: 'good' | 'warn' | 'bad' | 'neutral' }
  | { k: 'json'; value: unknown };

export interface NamedField {
  name: string;
  field: Field;
}

export interface DecodedAction {
  kind: string;
  label: string;
  /** One-line human summary, e.g. "Send 0.05 BTC.BTC to 3a1f…". */
  summary: string;
  fields: NamedField[];
  /** Plain-words warning shown for actions that move or lock funds. */
  warning?: string;
}

export const ACTION_LABELS: Record<string, string> = {
  Transfer: 'Transfer',
  PlaceOrder: 'Place order',
  CancelOrder: 'Cancel order',
  HouseQuote: 'House quote',
  BuyBudget: 'Buy action budget',
  CreateOffer: 'Create offer',
  UpdateOffer: 'Update offer',
  PauseOffer: 'Pause offer',
  CloseOffer: 'Close offer',
  StartTrade: 'Start trade',
  MarkPaid: 'Mark paid',
  ReleaseTrade: 'Release escrow',
  CancelTrade: 'Cancel trade',
  OpenDispute: 'Open dispute',
  SubmitEvidence: 'Submit evidence',
  RuleDispute: 'Rule dispute',
  RequestDepositAddress: 'Request deposit address',
  Withdraw: 'Withdraw',
  MintStable: 'Mint KUSD',
  BurnStable: 'Redeem KUSD',
  Bond: 'Bond',
  Unbond: 'Unbond',
  Delegate: 'Delegate',
  Undelegate: 'Undelegate',
  ClaimRewards: 'Claim rewards',
  Propose: 'Propose',
  Vote: 'Vote',
  ExecuteProposal: 'Execute proposal',
  Attest: 'Attest tier',
  SetParam: 'Set parameter',
  LockBudget: 'Lock budget',
  UnlockBudget: 'Unlock budget',
  AuthorizeSessionKey: 'Authorize session key',
  RevokeSessionKey: 'Revoke session key',
};

export function actionLabel(kind: string): string {
  return ACTION_LABELS[kind] ?? sentence(kind);
}

const MOVES_FUNDS: Record<string, string> = {
  Transfer: 'This sends funds out of your account.',
  Withdraw: 'This sends funds out of the chain to an external address.',
  ReleaseTrade: 'This releases escrowed funds to the buyer. Only approve after you received the payment.',
  LockBudget: 'This locks KEEL in your budget; it can be unlocked later.',
  StartTrade: 'This locks the trade amount in escrow.',
  MintStable: 'This converts reserve assets into KUSD.',
  BurnStable: 'This converts KUSD into a reserve asset.',
  Bond: 'This locks KEEL as a bond.',
  Delegate: 'This delegates KEEL to a validator.',
};

function str(v: unknown): string {
  if (v === null || v === undefined) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean' || typeof v === 'bigint') return String(v);
  if (Array.isArray(v) && v.every((x) => typeof x === 'number')) return (v as number[]).map((x) => x.toString(16).padStart(2, '0')).join('');
  return JSON.stringify(v);
}

function amountStr(v: unknown): string {
  if (typeof v === 'number') return String(Math.trunc(v));
  if (typeof v === 'bigint') return v.toString();
  return str(v);
}

function present(v: unknown): boolean {
  return v !== null && v !== undefined;
}

function amount(value: unknown, asset: string): Field {
  const raw = amountStr(value);
  return { k: 'amount', value: raw, asset, formatted: `${formatAmount(raw, decimalsOf(asset))} ${asset}` };
}

function price(value: unknown, pair: string): Field {
  const raw = amountStr(value);
  const { quote } = splitPair(pair);
  return { k: 'price', value: raw, pair, formatted: `${formatAmount(raw, decimalsOf(quote))} ${quote}` };
}

function id(what: 'offer' | 'trade' | 'order' | 'proposal', v: unknown): Field {
  return { k: 'id', what, id: amountStr(v) };
}

function genericFields(p: Record<string, unknown>): NamedField[] {
  return Object.entries(p).map(([name, value]) => ({
    name: sentence(name),
    field: typeof value === 'object' && value !== null ? { k: 'json', value } : typeof value === 'boolean' ? { k: 'bool', value } : { k: 'text', value: str(value) },
  }));
}

function offerSpecFields(spec: Record<string, unknown>): NamedField[] {
  const asset = str(spec['asset']);
  const sell = str(spec['side']) === 'sell';
  const out: NamedField[] = [
    { name: 'Side', field: { k: 'badge', value: sell ? 'Sell' : 'Buy', tone: sell ? 'bad' : 'good' } },
    { name: 'Asset', field: { k: 'asset', value: asset } },
    { name: 'Fiat', field: { k: 'text', value: str(spec['fiat_currency']) } },
    { name: 'Payment method', field: { k: 'text', value: str(spec['payment_method']) } },
    { name: 'Margin', field: { k: 'text', value: `${(Number(spec['margin_bps'] ?? 0) / 100).toFixed(2)}%` } },
  ];
  if (present(spec['fixed_price'])) out.push({ name: 'Fixed price (fiat minor units)', field: { k: 'number', value: amountStr(spec['fixed_price']) } });
  out.push({ name: 'Min amount', field: amount(spec['min_amount'], asset) });
  out.push({ name: 'Max amount', field: amount(spec['max_amount'], asset) });
  const win = Number(spec['payment_window_secs'] ?? 0);
  out.push({ name: 'Payment window', field: { k: 'duration', seconds: win, formatted: formatDuration(win) } });
  if (spec['country']) out.push({ name: 'Country', field: { k: 'text', value: str(spec['country']) } });
  out.push({ name: 'Minimum tier', field: { k: 'number', value: str(spec['min_tier']) } });
  if (spec['terms']) out.push({ name: 'Terms', field: { k: 'text', value: str(spec['terms']) } });
  if (present(spec['instructions_hash'])) out.push({ name: 'Instructions hash', field: { k: 'hash', value: str(spec['instructions_hash']) } });
  return out;
}

/** Plain-words description of what a session scope allows. */
export function scopeWords(scopes: readonly SessionScope[]): string {
  const parts: string[] = [];
  if (scopes.includes('markets')) parts.push('place and cancel orders');
  if (scopes.includes('p2p_manage')) parts.push('create, update, pause and close P2P offers and mark trades as paid');
  return parts.join(', and ');
}

/** "Allow this site to place and cancel orders for you until 9 Sep 2026, 14:05. It can never move funds." */
export function sessionSentence(scopes: readonly SessionScope[], expiresAt: number, who = 'this site'): string {
  return `Allow ${who} to ${scopeWords(scopes)} for you until ${formatDate(expiresAt)}. It can never move funds.`;
}

export function decodeAction(action: unknown): DecodedAction {
  const [kind, p] = untag(action);
  const label = actionLabel(kind);
  const view = (summary: string, fields: NamedField[]): DecodedAction => {
    const warning = MOVES_FUNDS[kind];
    return warning ? { kind, label, summary, fields, warning } : { kind, label, summary, fields };
  };

  switch (kind) {
    case 'Transfer': {
      const asset = str(p['asset']);
      const to = str(p['to']);
      const fields: NamedField[] = [
        { name: 'To', field: { k: 'address', value: to } },
        { name: 'Amount', field: amount(p['amount'], asset) },
      ];
      if (p['memo']) fields.push({ name: 'Memo', field: { k: 'text', value: str(p['memo']) } });
      return view(`Send ${formatAmount(amountStr(p['amount']), decimalsOf(asset))} ${asset} to ${to.slice(0, 8)}…${to.slice(-6)}`, fields);
    }
    case 'PlaceOrder': {
      const pair = str(p['pair']);
      const side = str(p['side']);
      const type = str(p['order_type']);
      const { base, quote } = splitPair(pair);
      const fields: NamedField[] = [
        { name: 'Pair', field: { k: 'pair', value: pair } },
        { name: 'Side', field: { k: 'badge', value: side === 'sell' ? 'Sell' : 'Buy', tone: side === 'sell' ? 'bad' : 'good' } },
        { name: 'Type', field: { k: 'badge', value: type === 'market' ? 'Market' : 'Limit', tone: 'neutral' } },
      ];
      if (present(p['price'])) fields.push({ name: 'Price', field: price(p['price'], pair) });
      if (present(p['quantity'])) fields.push({ name: 'Quantity', field: amount(p['quantity'], base) });
      if (present(p['quote_budget'])) fields.push({ name: 'Quote budget', field: amount(p['quote_budget'], quote) });
      if (present(p['client_id'])) fields.push({ name: 'Client id', field: { k: 'number', value: amountStr(p['client_id']) } });
      const qty = present(p['quantity']) ? `${formatAmount(amountStr(p['quantity']), decimalsOf(base))} ${base}` : present(p['quote_budget']) ? `up to ${formatAmount(amountStr(p['quote_budget']), decimalsOf(quote))} ${quote} of ${base}` : base;
      const at = present(p['price']) ? ` at ${formatAmount(amountStr(p['price']), decimalsOf(quote))} ${quote}` : ' at market';
      return view(`${side === 'sell' ? 'Sell' : 'Buy'} ${qty}${at} on ${pair}`, fields);
    }
    case 'CancelOrder':
      return view(`Cancel order #${amountStr(p['order_id'])}`, [{ name: 'Order', field: id('order', p['order_id']) }]);
    case 'BuyBudget':
      return view(`Buy ${amountStr(p['actions'])} actions of budget`, [{ name: 'Actions', field: { k: 'number', value: amountStr(p['actions']) } }]);
    case 'CreateOffer':
      return view(`Create ${str(p['side'])} offer for ${str(p['asset'])}`, offerSpecFields(p));
    case 'UpdateOffer': {
      const spec = (p['spec'] as Record<string, unknown> | undefined) ?? {};
      return view(`Update offer #${amountStr(p['offer_id'])}`, [{ name: 'Offer', field: id('offer', p['offer_id']) }, ...offerSpecFields(spec)]);
    }
    case 'PauseOffer': {
      const paused = Boolean(p['paused']);
      return { kind, label: paused ? 'Pause offer' : 'Resume offer', summary: `${paused ? 'Pause' : 'Resume'} offer #${amountStr(p['offer_id'])}`, fields: [{ name: 'Offer', field: id('offer', p['offer_id']) }, { name: 'Paused', field: { k: 'bool', value: paused } }] };
    }
    case 'CloseOffer':
      return view(`Close offer #${amountStr(p['offer_id'])}`, [{ name: 'Offer', field: id('offer', p['offer_id']) }]);
    case 'StartTrade':
      return view(`Start trade on offer #${amountStr(p['offer_id'])}`, [
        { name: 'Offer', field: id('offer', p['offer_id']) },
        { name: 'Amount (smallest units)', field: { k: 'number', value: amountStr(p['amount']) } },
        { name: 'Fiat amount (minor units)', field: { k: 'number', value: amountStr(p['fiat_amount']) } },
        { name: 'Instructions hash', field: { k: 'hash', value: str(p['instructions_hash']) } },
      ]);
    case 'MarkPaid': {
      const fields: NamedField[] = [{ name: 'Trade', field: id('trade', p['trade_id']) }];
      if (present(p['proof_hash'])) fields.push({ name: 'Payment proof hash', field: { k: 'hash', value: str(p['proof_hash']) } });
      return view(`Mark trade #${amountStr(p['trade_id'])} as paid`, fields);
    }
    case 'ReleaseTrade':
      return view(`Release escrow of trade #${amountStr(p['trade_id'])}`, [{ name: 'Trade', field: id('trade', p['trade_id']) }]);
    case 'CancelTrade':
      return view(`Cancel trade #${amountStr(p['trade_id'])}`, [{ name: 'Trade', field: id('trade', p['trade_id']) }]);
    case 'OpenDispute':
    case 'SubmitEvidence':
      return view(`${label} on trade #${amountStr(p['trade_id'])}`, [
        { name: 'Trade', field: id('trade', p['trade_id']) },
        { name: 'Evidence hash', field: { k: 'hash', value: str(p['evidence_hash']) } },
      ]);
    case 'RequestDepositAddress':
      return view(`Request ${str(p['chain'])} deposit address`, [{ name: 'Chain', field: { k: 'text', value: str(p['chain']) } }]);
    case 'Withdraw': {
      const asset = str(p['asset']);
      const chain = asset.split('.')[0] ?? '';
      return view(`Withdraw ${formatAmount(amountStr(p['amount']), decimalsOf(asset))} ${asset} to ${str(p['to'])}`, [
        { name: 'Asset', field: { k: 'asset', value: asset } },
        { name: 'Amount', field: amount(p['amount'], asset) },
        { name: 'Destination', field: { k: 'external', chain, value: str(p['to']) } },
      ]);
    }
    case 'MintStable':
      return view(`Mint KUSD from ${str(p['asset'])}`, [
        { name: 'Reserve asset', field: { k: 'asset', value: str(p['asset']) } },
        { name: 'Amount', field: amount(p['amount'], str(p['asset'])) },
      ]);
    case 'BurnStable':
      return view(`Redeem KUSD into ${str(p['asset'])}`, [
        { name: 'Burned', field: amount(p['amount'], 'KUSD') },
        { name: 'Redeemed into', field: { k: 'asset', value: str(p['asset']) } },
      ]);
    case 'Bond': {
      const fields: NamedField[] = [
        { name: 'Role', field: { k: 'badge', value: str(p['role']), tone: 'neutral' } },
        { name: 'Amount', field: amount(p['amount'], 'KEEL') },
      ];
      if (present(p['consensus_key'])) fields.push({ name: 'Consensus key', field: { k: 'hash', value: str(p['consensus_key']) } });
      return view(`Bond ${formatAmount(amountStr(p['amount']), 6)} KEEL as ${str(p['role']).toLowerCase()}`, fields);
    }
    case 'Unbond':
      return view(`Unbond ${formatAmount(amountStr(p['amount']), 6)} KEEL (${str(p['role']).toLowerCase()})`, [
        { name: 'Role', field: { k: 'badge', value: str(p['role']), tone: 'neutral' } },
        { name: 'Amount', field: amount(p['amount'], 'KEEL') },
      ]);
    case 'Delegate':
    case 'Undelegate':
      return view(`${label} ${formatAmount(amountStr(p['amount']), 6)} KEEL`, [
        { name: 'Validator', field: { k: 'address', value: str(p['validator']) } },
        { name: 'Amount', field: amount(p['amount'], 'KEEL') },
      ]);
    case 'ClaimRewards':
      return view('Claim staking rewards', []);
    case 'Vote': {
      const choice = str(p['choice']);
      const tone = choice === 'Yes' ? 'good' : choice === 'Abstain' ? 'neutral' : choice === 'Veto' ? 'bad' : 'warn';
      return view(`Vote ${choice.toLowerCase()} on proposal #${amountStr(p['proposal_id'])}`, [
        { name: 'Proposal', field: id('proposal', p['proposal_id']) },
        { name: 'Choice', field: { k: 'badge', value: choice === 'Veto' ? 'No with veto' : choice, tone } },
      ]);
    }
    case 'ExecuteProposal':
      return view(`Execute proposal #${amountStr(p['proposal_id'])}`, [{ name: 'Proposal', field: id('proposal', p['proposal_id']) }]);
    case 'Propose':
      return view(`Propose: ${str(p['title'])}`, [
        { name: 'Title', field: { k: 'text', value: str(p['title']) } },
        { name: 'Description', field: { k: 'text', value: str(p['description']) } },
        { name: 'Kind', field: { k: 'json', value: p['kind'] } },
      ]);
    case 'Attest':
      return view(`Attest tier ${str(p['tier'])}`, [
        { name: 'Subject', field: { k: 'address', value: str(p['subject']) } },
        { name: 'Tier', field: { k: 'number', value: str(p['tier']) } },
        { name: 'Expires', field: { k: 'timestamp', value: Number(p['expires_at']), formatted: formatDate(Number(p['expires_at'])) } },
      ]);
    case 'LockBudget':
      return view(`Lock ${formatAmount(amountStr(p['amount']), 6)} KEEL as action budget`, [{ name: 'Amount', field: amount(p['amount'], 'KEEL') }]);
    case 'UnlockBudget':
      return view(`Unlock ${formatAmount(amountStr(p['amount']), 6)} KEEL of action budget`, [{ name: 'Amount', field: amount(p['amount'], 'KEEL') }]);
    case 'AuthorizeSessionKey': {
      const bits = Number(p['scope'] ?? 0);
      const scopes = scopesFromBits(bits);
      const exp = Number(p['expires_at'] ?? 0);
      return view(sessionSentence(scopes, exp, 'the session key'), [
        { name: 'Session key', field: { k: 'address', value: str(p['key']) } },
        { name: 'Allowed to', field: { k: 'text', value: scopes.length ? scopeWords(scopes) : `nothing (scope ${bits})` } },
        { name: 'Scope bits', field: { k: 'number', value: String(bits) } },
        { name: 'Expires', field: { k: 'timestamp', value: exp, formatted: formatDate(exp) } },
        { name: 'Can move funds', field: { k: 'badge', value: 'Never', tone: 'good' } },
      ]);
    }
    case 'RevokeSessionKey':
      return view('Revoke a session key', [{ name: 'Session key', field: { k: 'address', value: str(p['key']) } }]);
    default:
      return view(sentence(kind), genericFields(p));
  }
}
