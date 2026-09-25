/**
 * Turns the decoded action JSON (serde of `keel_actions::Action`, externally
 * tagged) and event records into labelled, typed fields the UI renders with
 * links. Unknown kinds fall back to a generic key/value listing so a newer
 * chain never breaks the explorer.
 */
import type { DecodedAction, EventRecord } from '../api/types';
import { chainCode } from './links';
import { sentence, splitPair } from './format';

export type Field =
  | { k: 'address'; value: string }
  | { k: 'amount'; value: string; asset: string }
  | { k: 'asset'; value: string }
  | { k: 'pair'; value: string }
  | { k: 'offer'; id: number }
  | { k: 'trade'; id: number }
  | { k: 'order'; id: number }
  | { k: 'proposal'; id: number }
  | { k: 'outbound'; id: number }
  | { k: 'validator'; value: string }
  | { k: 'hash'; value: string }
  | { k: 'external'; chain: string; value: string; what: 'tx' | 'address' }
  | { k: 'chain'; value: string }
  | { k: 'height'; value: number }
  | { k: 'timestamp'; value: number }
  | { k: 'duration'; seconds: number }
  | { k: 'bps'; value: number }
  | { k: 'price'; value: string; pair: string }
  | { k: 'number'; value: number | string }
  | { k: 'bool'; value: boolean }
  | { k: 'text'; value: string }
  | { k: 'badge'; value: string; tone?: 'good' | 'warn' | 'bad' | 'neutral' }
  | { k: 'json'; value: unknown };

export interface NamedField {
  name: string;
  field: Field;
}

export interface DecodedView {
  kind: string;
  label: string;
  /** One-line human summary used in lists and page titles. */
  summary: string;
  fields: NamedField[];
  /** Fee explanation in the chain's own terms. */
  feeNote: string;
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
  ObserveDeposit: 'Observe deposit',
  ObserveOutbound: 'Observe outbound',
  ReportNetworkFee: 'Report network fee',
  Withdraw: 'Withdraw',
  RegisterVault: 'Register vault',
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
  EpochBoundary: 'Epoch boundary',
};

export const MODULE_LABELS: Record<string, string> = {
  tokens: 'Tokens',
  markets: 'Order book',
  budgets: 'Budgets',
  market_p2p: 'P2P offers',
  disputes: 'Disputes',
  vaults: 'Vaults',
  stable: 'Stablecoin',
  staking: 'Staking',
  gov: 'Governance',
  attest: 'Attestations',
};

export function actionLabel(kind: string): string {
  return ACTION_LABELS[kind] ?? sentence(kind);
}

const NO_FEE = 'No fee. Actions carry no gas; this one consumed one unit of the signer’s action budget.';

function feeNoteFor(kind: string): string {
  switch (kind) {
    case 'PlaceOrder':
      return 'No fee to place. A taker fee (basis points, governance parameter) is taken only from what the taker receives on each fill; maker fee is 0 at launch.';
    case 'ReleaseTrade':
      return 'The seller fee (basis points, with a surcharge under the small-trade threshold) is taken from the escrow on release and split between treasury, validator rewards and burn.';
    case 'Withdraw':
      return 'The actual network fee of the batched outbound is passed through plus a flat withdrawal fee (governance parameter). No gas.';
    case 'RuleDispute':
      return 'The dispute fee is charged to the losing side out of the escrow. No gas.';
    case 'BuyBudget':
      return 'Paid in KEEL to the treasury; buys additional action budget for the signer. No gas.';
    case 'Propose':
      return 'Locks the proposal deposit in KEEL; returned unless the proposal is vetoed. No gas.';
    case 'CreateOffer':
      return 'Locks the refundable KEEL offer deposit for the life of the offer. No gas.';
    case 'ObserveDeposit':
    case 'ObserveOutbound':
    case 'ReportNetworkFee':
    case 'RegisterVault':
      return 'Observer action: budget-free, backed by the observer bond (slashable for false observations).';
    case 'EpochBoundary':
      return 'System action at the epoch boundary: no signer fee; rewards are distributed pro rata.';
    default:
      return NO_FEE;
  }
}

function str(v: unknown): string {
  if (v === null || v === undefined) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean' || typeof v === 'bigint') return String(v);
  return JSON.stringify(v);
}

function num(v: unknown): number {
  return typeof v === 'number' ? v : Number(v ?? 0);
}

function amountStr(v: unknown): string {
  if (typeof v === 'number') return Number.isInteger(v) ? String(v) : String(Math.trunc(v));
  return str(v);
}

/** Splits an externally tagged enum into [variant, payload]. */
export function untag(value: unknown): [string, Record<string, unknown>] {
  if (typeof value === 'string') return [value, {}];
  if (typeof value === 'object' && value !== null) {
    const keys = Object.keys(value);
    if (keys.length === 1) {
      const k = keys[0]!;
      const payload = (value as Record<string, unknown>)[k];
      if (typeof payload === 'object' && payload !== null && !Array.isArray(payload)) return [k, payload as Record<string, unknown>];
      return [k, { value: payload }];
    }
    // Already flattened ({kind: 'Transfer', ...}).
    const rec = value as Record<string, unknown>;
    if (typeof rec['kind'] === 'string') {
      const { kind, ...rest } = rec;
      return [kind as string, rest];
    }
    if (typeof rec['type'] === 'string') {
      const { type, ...rest } = rec;
      return [type as string, rest];
    }
  }
  return ['Unknown', { value }];
}

function genericFields(p: Record<string, unknown>): NamedField[] {
  return Object.entries(p).map(([name, value]) => ({ name: sentence(name), field: typeof value === 'object' && value !== null ? { k: 'json', value } : typeof value === 'boolean' ? { k: 'bool', value } : { k: 'text', value: str(value) } }));
}

function offerSpecFields(spec: Record<string, unknown>): NamedField[] {
  const asset = str(spec['asset']);
  const out: NamedField[] = [
    { name: 'Side', field: { k: 'badge', value: str(spec['side']) === 'sell' ? 'Sell' : 'Buy', tone: str(spec['side']) === 'sell' ? 'bad' : 'good' } },
    { name: 'Asset', field: { k: 'asset', value: asset } },
    { name: 'Fiat', field: { k: 'text', value: str(spec['fiat_currency']) } },
    { name: 'Payment method', field: { k: 'text', value: str(spec['payment_method']) } },
    { name: 'Margin', field: { k: 'bps', value: num(spec['margin_bps']) } },
  ];
  if (spec['fixed_price'] !== null && spec['fixed_price'] !== undefined) out.push({ name: 'Fixed price (fiat minor units)', field: { k: 'number', value: amountStr(spec['fixed_price']) } });
  out.push({ name: 'Min amount', field: { k: 'amount', value: amountStr(spec['min_amount']), asset } });
  out.push({ name: 'Max amount', field: { k: 'amount', value: amountStr(spec['max_amount']), asset } });
  out.push({ name: 'Payment window', field: { k: 'duration', seconds: num(spec['payment_window_secs']) } });
  if (spec['country']) out.push({ name: 'Country', field: { k: 'text', value: str(spec['country']) } });
  out.push({ name: 'Minimum tier', field: { k: 'number', value: num(spec['min_tier']) } });
  if (spec['terms']) out.push({ name: 'Terms', field: { k: 'text', value: str(spec['terms']) } });
  if (spec['instructions_hash']) out.push({ name: 'Instructions hash', field: { k: 'hash', value: str(spec['instructions_hash']) } });
  return out;
}

function proposalKindFields(kind: unknown): NamedField[] {
  const [variant, p] = untag(kind);
  const out: NamedField[] = [{ name: 'Proposal type', field: { k: 'badge', value: sentence(variant), tone: 'neutral' } }];
  switch (variant) {
    case 'ParamChange':
      out.push({ name: 'Parameter', field: { k: 'text', value: str(p['key']) } }, { name: 'New value', field: { k: 'number', value: amountStr(p['value']) } });
      break;
    case 'ListPair':
      out.push({ name: 'Pair', field: { k: 'pair', value: str(p['symbol']) } }, { name: 'Taker fee', field: { k: 'bps', value: num(p['taker_fee_bps']) } }, { name: 'Maker fee', field: { k: 'bps', value: num(p['maker_fee_bps']) } }, { name: 'Config', field: { k: 'json', value: p } });
      break;
    case 'DelistPair':
      out.push({ name: 'Pair', field: { k: 'pair', value: str(p['symbol']) } });
      break;
    case 'RegisterAsset':
      out.push({ name: 'Asset', field: { k: 'asset', value: str(p['asset']) } }, { name: 'Decimals', field: { k: 'number', value: num(p['decimals']) } });
      break;
    case 'TreasurySpend':
      out.push({ name: 'Recipient', field: { k: 'address', value: str(p['to']) } }, { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: str(p['asset']) } });
      break;
    case 'SoftwareUpgrade':
      out.push({ name: 'Version', field: { k: 'text', value: str(p['version']) } }, { name: 'Halt height', field: { k: 'height', value: num(p['height']) } });
      break;
    case 'SetArbitrators':
    case 'SetAttesters':
    case 'SetObservers':
      (Array.isArray(p['members']) ? (p['members'] as unknown[]) : []).forEach((m, i) => out.push({ name: `Member ${i + 1}`, field: { k: 'address', value: str(m) } }));
      if (p['threshold'] !== undefined) out.push({ name: 'Threshold', field: { k: 'number', value: num(p['threshold']) } });
      break;
    case 'SetStableBasket':
      out.push({ name: 'Reserve asset', field: { k: 'asset', value: str(p['asset']) } }, { name: 'Cap', field: { k: 'amount', value: amountStr(p['cap']), asset: str(p['asset']) } }, { name: 'Enabled', field: { k: 'bool', value: Boolean(p['enabled']) } });
      break;
    case 'PauseModule':
      out.push({ name: 'Module', field: { k: 'text', value: str(p['module']) } }, { name: 'Until height', field: { k: 'height', value: num(p['until_height']) } });
      break;
    case 'SetParamAdmin':
      out.push({ name: 'Admin', field: p['admin'] ? { k: 'address', value: str(p['admin']) } : { k: 'badge', value: 'Revoked', tone: 'warn' } });
      break;
    case 'Text':
      break;
    default:
      out.push(...genericFields(p));
  }
  return out;
}

export function decodeAction(action: DecodedAction | undefined, kindHint?: string): DecodedView {
  const [variant, p] = action === undefined ? [kindHint ?? 'Unknown', {}] : untag(action);
  const kind = variant === 'Unknown' && kindHint ? kindHint : variant;
  const label = actionLabel(kind);
  const feeNote = feeNoteFor(kind);
  const view = (summary: string, fields: NamedField[]): DecodedView => ({ kind, label, summary, fields, feeNote });

  switch (kind) {
    case 'Transfer': {
      const asset = str(p['asset']);
      const fields: NamedField[] = [
        { name: 'To', field: { k: 'address', value: str(p['to']) } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset } },
      ];
      if (p['memo']) fields.push({ name: 'Memo', field: { k: 'text', value: str(p['memo']) } });
      return view(`Transfer ${asset}`, fields);
    }
    case 'PlaceOrder': {
      const pair = str(p['pair']);
      const side = str(p['side']);
      const type = str(p['order_type']);
      const fields: NamedField[] = [
        { name: 'Pair', field: { k: 'pair', value: pair } },
        { name: 'Side', field: { k: 'badge', value: side === 'sell' ? 'Sell' : 'Buy', tone: side === 'sell' ? 'bad' : 'good' } },
        { name: 'Type', field: { k: 'badge', value: type === 'market' ? 'Market' : 'Limit', tone: 'neutral' } },
      ];
      const { base, quote } = splitPairLocal(pair);
      if (p['price'] !== null && p['price'] !== undefined) fields.push({ name: 'Price', field: { k: 'price', value: amountStr(p['price']), pair } });
      if (p['quantity'] !== null && p['quantity'] !== undefined) fields.push({ name: 'Quantity', field: { k: 'amount', value: amountStr(p['quantity']), asset: base } });
      if (p['quote_budget'] !== null && p['quote_budget'] !== undefined) fields.push({ name: 'Quote budget', field: { k: 'amount', value: amountStr(p['quote_budget']), asset: quote } });
      if (p['client_id'] !== null && p['client_id'] !== undefined) fields.push({ name: 'Client id', field: { k: 'number', value: num(p['client_id']) } });
      return view(`${type === 'market' ? 'Market' : 'Limit'} ${side} on ${pair}`, fields);
    }
    case 'CancelOrder':
      return view(`Cancel order #${str(p['order_id'])}`, [{ name: 'Order', field: { k: 'order', id: num(p['order_id']) } }]);
    case 'HouseQuote': {
      const pair = str(p['pair']);
      const { base } = splitPairLocal(pair);
      const fields: NamedField[] = [{ name: 'Pair', field: { k: 'pair', value: pair } }];
      const bid = p['bid'] as [unknown, unknown] | null | undefined;
      const ask = p['ask'] as [unknown, unknown] | null | undefined;
      if (bid) fields.push({ name: 'Bid price', field: { k: 'price', value: amountStr(bid[0]), pair } }, { name: 'Bid size', field: { k: 'amount', value: amountStr(bid[1]), asset: base } });
      if (ask) fields.push({ name: 'Ask price', field: { k: 'price', value: amountStr(ask[0]), pair } }, { name: 'Ask size', field: { k: 'amount', value: amountStr(ask[1]), asset: base } });
      fields.push({ name: 'Valid until', field: { k: 'height', value: num(p['valid_until']) } });
      return view(`House quote on ${pair}`, fields);
    }
    case 'BuyBudget':
      return view(`Buy ${str(p['actions'])} actions of budget`, [{ name: 'Actions', field: { k: 'number', value: num(p['actions']) } }]);
    case 'CreateOffer':
      return view(`Create ${str(p['side'])} offer for ${str(p['asset'])}`, offerSpecFields(p));
    case 'UpdateOffer': {
      const spec = (p['spec'] as Record<string, unknown> | undefined) ?? {};
      return view(`Update offer #${str(p['offer_id'])}`, [{ name: 'Offer', field: { k: 'offer', id: num(p['offer_id']) } }, ...offerSpecFields(spec)]);
    }
    case 'PauseOffer': {
      const paused = Boolean(p['paused']);
      return { kind, label: paused ? 'Pause offer' : 'Resume offer', summary: `${paused ? 'Pause' : 'Resume'} offer #${str(p['offer_id'])}`, fields: [{ name: 'Offer', field: { k: 'offer', id: num(p['offer_id']) } }, { name: 'Paused', field: { k: 'bool', value: paused } }], feeNote };
    }
    case 'CloseOffer':
      return view(`Close offer #${str(p['offer_id'])}`, [{ name: 'Offer', field: { k: 'offer', id: num(p['offer_id']) } }]);
    case 'StartTrade':
      return view(`Start trade on offer #${str(p['offer_id'])}`, [
        { name: 'Offer', field: { k: 'offer', id: num(p['offer_id']) } },
        { name: 'Amount (smallest units)', field: { k: 'number', value: amountStr(p['amount']) } },
        { name: 'Fiat amount (minor units)', field: { k: 'number', value: amountStr(p['fiat_amount']) } },
        { name: 'Instructions hash', field: { k: 'hash', value: str(p['instructions_hash']) } },
      ]);
    case 'MarkPaid': {
      const fields: NamedField[] = [{ name: 'Trade', field: { k: 'trade', id: num(p['trade_id']) } }];
      if (p['proof_hash']) fields.push({ name: 'Payment proof hash', field: { k: 'hash', value: str(p['proof_hash']) } });
      return view(`Mark trade #${str(p['trade_id'])} paid`, fields);
    }
    case 'ReleaseTrade':
      return view(`Release escrow of trade #${str(p['trade_id'])}`, [{ name: 'Trade', field: { k: 'trade', id: num(p['trade_id']) } }]);
    case 'CancelTrade':
      return view(`Cancel trade #${str(p['trade_id'])}`, [{ name: 'Trade', field: { k: 'trade', id: num(p['trade_id']) } }]);
    case 'OpenDispute':
    case 'SubmitEvidence':
      return view(`${label} on trade #${str(p['trade_id'])}`, [
        { name: 'Trade', field: { k: 'trade', id: num(p['trade_id']) } },
        { name: 'Evidence hash', field: { k: 'hash', value: str(p['evidence_hash']) } },
      ]);
    case 'RuleDispute': {
      const [rv, rp] = untag(p['ruling']);
      const fields: NamedField[] = [{ name: 'Trade', field: { k: 'trade', id: num(p['trade_id']) } }];
      if (rv === 'Split') fields.push({ name: 'Ruling', field: { k: 'badge', value: 'Split', tone: 'warn' } }, { name: 'Buyer share', field: { k: 'bps', value: num(rp['buyer_bps']) } });
      else fields.push({ name: 'Ruling', field: { k: 'badge', value: rv === 'WinsBuyer' ? 'Wins buyer' : rv === 'WinsSeller' ? 'Wins seller' : sentence(rv), tone: 'neutral' } });
      return view(`Rule dispute on trade #${str(p['trade_id'])}`, fields);
    }
    case 'RequestDepositAddress':
      return view(`Request ${chainCode(str(p['chain']))} deposit address`, [{ name: 'Chain', field: { k: 'chain', value: chainCode(str(p['chain'])) } }]);
    case 'ObserveDeposit': {
      const chain = chainCode(str(p['chain']));
      const [proofKind] = untag(p['proof'] ?? 'None');
      return view(`Observe ${chain} deposit`, [
        { name: 'Chain', field: { k: 'chain', value: chain } },
        { name: 'Asset', field: { k: 'asset', value: str(p['asset']) } },
        { name: 'External tx', field: { k: 'external', chain, value: str(p['tx_hash']), what: 'tx' } },
        { name: 'Output / log index', field: { k: 'number', value: num(p['index']) } },
        { name: 'Deposit address index', field: { k: 'number', value: num(p['deposit_index']) } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: str(p['asset']) } },
        { name: 'External height', field: { k: 'number', value: num(p['external_height']) } },
        { name: 'Observer tip', field: { k: 'number', value: num(p['tip_height']) } },
        { name: 'Depth', field: { k: 'number', value: num(p['tip_height']) - num(p['external_height']) } },
        { name: 'Proof', field: { k: 'badge', value: proofKind === 'None' ? 'Attestation only' : `${proofKind} light-client proof`, tone: proofKind === 'None' ? 'warn' : 'good' } },
      ]);
    }
    case 'ObserveOutbound':
      return view(`Observe outbound #${str(p['outbound_id'])}`, [
        { name: 'Outbound', field: { k: 'outbound', id: num(p['outbound_id']) } },
        { name: 'External tx', field: { k: 'hash', value: str(p['tx_hash']) } },
        { name: 'External height', field: { k: 'number', value: num(p['external_height']) } },
        { name: 'Fee paid (native units)', field: { k: 'number', value: amountStr(p['fee_paid']) } },
        { name: 'Success', field: { k: 'bool', value: Boolean(p['success']) } },
      ]);
    case 'ReportNetworkFee':
      return view(`Report ${chainCode(str(p['chain']))} fee rate`, [
        { name: 'Chain', field: { k: 'chain', value: chainCode(str(p['chain'])) } },
        { name: 'Fee rate', field: { k: 'number', value: num(p['fee_rate']) } },
      ]);
    case 'Withdraw': {
      const asset = str(p['asset']);
      const chain = asset.split('.')[0] ?? '';
      return view(`Withdraw ${asset}`, [
        { name: 'Asset', field: { k: 'asset', value: asset } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset } },
        { name: 'Destination', field: { k: 'external', chain, value: str(p['to']), what: 'address' } },
      ]);
    }
    case 'RegisterVault':
      return view(`Register ${chainCode(str(p['chain']))} vault for epoch ${str(p['epoch'])}`, [
        { name: 'Chain', field: { k: 'chain', value: chainCode(str(p['chain'])) } },
        { name: 'Epoch', field: { k: 'number', value: num(p['epoch']) } },
        { name: 'Threshold', field: { k: 'number', value: num(p['threshold']) } },
        ...(Array.isArray(p['signers']) ? (p['signers'] as unknown[]).map((s, i): NamedField => ({ name: `Signer ${i + 1}`, field: { k: 'address', value: str(s) } })) : []),
      ]);
    case 'MintStable':
      return view(`Mint KUSD from ${str(p['asset'])}`, [
        { name: 'Reserve asset', field: { k: 'asset', value: str(p['asset']) } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: str(p['asset']) } },
        { name: 'Minted', field: { k: 'amount', value: amountStr(p['amount']), asset: 'KUSD' } },
      ]);
    case 'BurnStable':
      return view(`Redeem KUSD into ${str(p['asset'])}`, [
        { name: 'Burned', field: { k: 'amount', value: amountStr(p['amount']), asset: 'KUSD' } },
        { name: 'Redeemed into', field: { k: 'asset', value: str(p['asset']) } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: str(p['asset']) } },
      ]);
    case 'Bond': {
      const fields: NamedField[] = [
        { name: 'Role', field: { k: 'badge', value: str(p['role']), tone: 'neutral' } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: 'KEEL' } },
      ];
      if (p['consensus_key']) fields.push({ name: 'Consensus key', field: { k: 'hash', value: str(p['consensus_key']) } });
      return view(`Bond as ${str(p['role']).toLowerCase()}`, fields);
    }
    case 'Unbond':
      return view(`Unbond ${str(p['role']).toLowerCase()} bond`, [
        { name: 'Role', field: { k: 'badge', value: str(p['role']), tone: 'neutral' } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: 'KEEL' } },
      ]);
    case 'Delegate':
    case 'Undelegate':
      return view(`${label} KEEL`, [
        { name: 'Validator', field: { k: 'validator', value: str(p['validator']) } },
        { name: 'Amount', field: { k: 'amount', value: amountStr(p['amount']), asset: 'KEEL' } },
      ]);
    case 'ClaimRewards':
      return view('Claim staking rewards', []);
    case 'Propose':
      return view(`Propose: ${str(p['title'])}`, [
        { name: 'Title', field: { k: 'text', value: str(p['title']) } },
        { name: 'Description', field: { k: 'text', value: str(p['description']) } },
        ...proposalKindFields(p['kind']),
      ]);
    case 'Vote': {
      const choice = str(p['choice']);
      const tone = choice === 'Yes' ? 'good' : choice === 'Abstain' ? 'neutral' : choice === 'Veto' ? 'bad' : 'warn';
      return view(`Vote ${choice.toLowerCase()} on proposal #${str(p['proposal_id'])}`, [
        { name: 'Proposal', field: { k: 'proposal', id: num(p['proposal_id']) } },
        { name: 'Choice', field: { k: 'badge', value: choice === 'Veto' ? 'No with veto' : choice, tone } },
      ]);
    }
    case 'ExecuteProposal':
      return view(`Execute proposal #${str(p['proposal_id'])}`, [{ name: 'Proposal', field: { k: 'proposal', id: num(p['proposal_id']) } }]);
    case 'Attest':
      return view(`Attest tier ${str(p['tier'])} for account`, [
        { name: 'Subject', field: { k: 'address', value: str(p['subject']) } },
        { name: 'Tier', field: { k: 'number', value: num(p['tier']) } },
        { name: 'Expires', field: { k: 'timestamp', value: num(p['expires_at']) * 1000 } },
      ]);
    case 'SetParam':
      return view(`Set ${str(p['key'])} = ${str(p['value'])}`, [
        { name: 'Parameter', field: { k: 'text', value: str(p['key']) } },
        { name: 'Value', field: { k: 'number', value: amountStr(p['value']) } },
      ]);
    case 'EpochBoundary':
      return view('Epoch boundary', []);
    default:
      return view(sentence(kind), genericFields(p));
  }
}

function splitPairLocal(pair: string): { base: string; quote: string } {
  return splitPair(pair);
}

// ---------------------------------------------------------------- events

export interface DecodedEvent {
  type: string;
  label: string;
  fields: NamedField[];
  tone: 'good' | 'warn' | 'bad' | 'neutral';
}

const ADDRESS_KEYS = new Set(['from', 'to', 'owner', 'buyer', 'seller', 'by', 'voter', 'proposer', 'subject', 'observer', 'validator', 'admin']);
const ASSET_KEYS = new Set(['asset', 'from_asset', 'into']);
const AMOUNT_KEYS = new Set(['amount', 'resting', 'quantity', 'quote', 'fee', 'released', 'paid', 'to_buyer', 'buyer_amount', 'seller_amount', 'weight', 'reserves', 'liabilities', 'refunded']);

function eventAssetOf(e: EventRecord): string | undefined {
  for (const k of ['asset', 'into', 'from']) {
    const v = e[k];
    if (typeof v === 'string' && /^[A-Z]{2,6}(\.[A-Z]{2,6})?$/.test(v) && !ADDRESS_KEYS.has(k)) return v;
  }
  if (typeof e['from'] === 'string' && e['type'] === 'StableMinted') return e['from'] as string;
  return undefined;
}

export function decodeEvent(e: EventRecord): DecodedEvent {
  const type = str(e['type']);
  const asset = eventAssetOf(e);
  const pair = typeof e['pair'] === 'string' ? (e['pair'] as string) : undefined;
  const fields: NamedField[] = [];
  for (const [key, value] of Object.entries(e)) {
    if (key === 'type') continue;
    const name = sentence(key);
    if (value === null || value === undefined) continue;
    if (type === 'StableMinted' && key === 'from') {
      fields.push({ name: 'Reserve asset', field: { k: 'asset', value: str(value) } });
      continue;
    }
    if (ADDRESS_KEYS.has(key) && typeof value === 'string') {
      fields.push({ name, field: key === 'validator' ? { k: 'validator', value } : { k: 'address', value } });
    } else if (ASSET_KEYS.has(key)) {
      fields.push({ name, field: { k: 'asset', value: str(value) } });
    } else if (key === 'pair') {
      fields.push({ name, field: { k: 'pair', value: str(value) } });
    } else if (key === 'price' && pair) {
      fields.push({ name, field: { k: 'price', value: amountStr(value), pair } });
    } else if (AMOUNT_KEYS.has(key)) {
      let a = asset;
      if (pair) {
        const { base, quote } = splitPairLocal(pair);
        a = key === 'quote' ? quote : key === 'fee' ? undefined : base;
      }
      if (key === 'weight' || key === 'paid' || (type.startsWith('Bond') && key === 'amount') || type === 'Delegated' || type === 'Undelegated' || type === 'Slashed' || type === 'Unbonded' || type === 'BondReleased' || type === 'VestingReleased') a = 'KEEL';
      fields.push({ name, field: a ? { k: 'amount', value: amountStr(value), asset: a } : { k: 'number', value: amountStr(value) } });
    } else if (key === 'trade_id') {
      fields.push({ name: 'Trade', field: { k: 'trade', id: num(value) } });
    } else if (key === 'offer_id') {
      fields.push({ name: 'Offer', field: { k: 'offer', id: num(value) } });
    } else if (key === 'order_id' || key === 'maker_order_id') {
      fields.push({ name, field: { k: 'order', id: num(value) } });
    } else if (key === 'proposal_id') {
      fields.push({ name: 'Proposal', field: { k: 'proposal', id: num(value) } });
    } else if (key === 'outbound_id') {
      fields.push({ name: 'Outbound', field: { k: 'outbound', id: num(value) } });
    } else if (key === 'tx_hash' && typeof e['chain'] === 'string') {
      fields.push({ name: 'External tx', field: { k: 'external', chain: chainCode(e['chain'] as string), value: str(value), what: 'tx' } });
    } else if (key === 'chain') {
      fields.push({ name, field: { k: 'chain', value: chainCode(str(value)) } });
    } else if (key.endsWith('_height') || key === 'height') {
      fields.push({ name, field: { k: 'height', value: num(value) } });
    } else if (typeof value === 'boolean') {
      fields.push({ name, field: { k: 'bool', value } });
    } else if (typeof value === 'object') {
      fields.push({ name, field: { k: 'json', value } });
    } else {
      fields.push({ name, field: { k: 'text', value: str(value) } });
    }
  }
  const tone: DecodedEvent['tone'] = /Rejected|Failed|Slashed|Breached|Halted|Cancelled|Dispute/.test(type) ? (/Breached|Slashed|Failed/.test(type) ? 'bad' : 'warn') : /Filled|Credited|Released|Executed|Confirmed|Resumed|Minted|Claimed|Distributed/.test(type) ? 'good' : 'neutral';
  return { type, label: sentence(type), fields, tone };
}
