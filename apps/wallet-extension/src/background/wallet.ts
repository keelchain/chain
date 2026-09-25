/**
 * The wallet controller: every provider request (from pages) and every UI
 * request (from the popup) ends up here. It has no chrome.* dependency so
 * the whole flow is unit-testable; background/index.ts wires it to the
 * browser.
 */
import { NETWORKS, findNetwork, type Network } from '../core/networks';
import type { KeyValueStore } from '../core/storage';
import { decryptVault, encryptVault, WrongPasswordError } from '../core/vault';
import { accountAddress, generateMnemonic, mnemonicToSeed, normalizeMnemonic, validateMnemonic } from '../core/keys';
import { sessionAction, validateEnvelope, validateSessionRequest } from '../core/actions';
import { decodeAction, sessionSentence, type DecodedAction } from '../core/decode';
import { signAction, signMessage } from '../core/sign';
import { fromWire, newId, toWire, WalletError, type ProviderEvent, type ProviderRequest } from '../core/protocol';
import type { AccountInfo, Envelope, SessionScope } from '../inpage/types';
import { Approvals, type ApprovalRequest, type NetworkRef } from './approvals';
import { Session } from './session';
import { loadState, saveState, type AccountMeta, type WalletState } from './state';

export interface WalletDeps {
  store: KeyValueStore;
  session: Session;
  approvals: Approvals;
  /** Deliver an event to one origin (or to every page when `origin` is null). */
  emit: (origin: string | null, event: ProviderEvent) => void;
  now?: () => number;
}

/** What the popup renders from. */
export interface UiState {
  hasVault: boolean;
  locked: boolean;
  remainingMs: number;
  network: Network;
  networks: readonly Network[];
  accounts: AccountMeta[];
  activeIndex: number;
  address: string | null;
  /** Origins connected on the current network. */
  connectedOrigins: string[];
  settings: WalletState['settings'];
  pending: ApprovalRequest[];
}

export class UiError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'UiError';
  }
}

function assertOrigin(origin: string): void {
  let u: URL;
  try {
    u = new URL(origin);
  } catch {
    throw new WalletError('INVALID_REQUEST', 'Bad origin');
  }
  if (u.origin !== origin || !/^https?:$/.test(u.protocol)) throw new WalletError('INVALID_REQUEST', 'Bad origin');
}

function rec(v: unknown): Record<string, unknown> {
  return typeof v === 'object' && v !== null ? (v as Record<string, unknown>) : {};
}

export class Wallet {
  private state: WalletState | null = null;
  private readonly now: () => number;

  constructor(private readonly deps: WalletDeps) {
    this.now = deps.now ?? (() => Date.now());
  }

  private async st(): Promise<WalletState> {
    if (!this.state) this.state = await loadState(this.deps.store);
    return this.state;
  }

  private async save(): Promise<void> {
    if (this.state) await saveState(this.deps.store, this.state);
  }

  private network(s: WalletState): Network {
    return findNetwork(s.networkId) ?? NETWORKS[0]!;
  }

  private networkRef(n: Network): NetworkRef {
    return { id: n.id, name: n.name, chainId: n.chainId };
  }

  private activeAccount(s: WalletState): AccountMeta {
    const a = s.accounts.find((x) => x.index === s.activeIndex) ?? s.accounts[0];
    if (!s.vault || !a) throw new WalletError('NO_ACCOUNT', 'Create or import a wallet first.');
    return a;
  }

  private info(s: WalletState): AccountInfo {
    const n = this.network(s);
    return { address: this.activeAccount(s).address, network: n.id, chainId: n.chainId };
  }

  private isConnected(s: WalletState, origin: string, networkId = s.networkId): boolean {
    return Boolean(s.connections[networkId]?.[origin]);
  }

  private requireSigningNetwork(s: WalletState): Network {
    const n = this.network(s);
    if (n.placeholder) throw new WalletError('WRONG_NETWORK', `${n.name} cannot sign yet. Switch the wallet to another network.`);
    return n;
  }

  private async approve(req: ApprovalRequest): Promise<void> {
    await this.deps.approvals.ask(req);
    this.deps.session.touch();
  }

  private requireKey(index: number) {
    const k = this.deps.session.keypair(index);
    if (!k) throw new WalletError('LOCKED');
    return k;
  }

  // ------------------------------------------------------------------ provider (pages)

  /** Handles one page request; params and result are wire JSON (bigint markers). */
  async handleProvider(origin: string, request: ProviderRequest): Promise<unknown> {
    assertOrigin(origin);
    const params = fromWire(request.params);
    switch (request.method) {
      case 'connect':
        return toWire(await this.connect(origin, params));
      case 'disconnect':
        await this.disconnect(origin);
        return null;
      case 'getAccount':
        return toWire(await this.getAccount(origin));
      case 'signMessage':
        return toWire(await this.signMessage(origin, params));
      case 'signAction':
        return toWire(await this.signAction(origin, params));
      case 'authorizeSession':
        return toWire(await this.authorizeSession(origin, params));
      default:
        throw new WalletError('INVALID_REQUEST', `Unknown method ${String(request.method)}`);
    }
  }

  async connect(origin: string, opts: unknown): Promise<AccountInfo> {
    const s = await this.st();
    const wanted = rec(opts)['network'];
    if (wanted !== undefined) {
      if (typeof wanted !== 'string' || !findNetwork(wanted)) throw new WalletError('INVALID_REQUEST', `Unknown network ${String(wanted)}`);
      if (wanted !== s.networkId) throw new WalletError('WRONG_NETWORK', `The wallet is on ${s.networkId}, the site wants ${wanted}. Switch the network in the wallet.`);
    }
    const account = this.activeAccount(s);
    const n = this.network(s);
    if (this.isConnected(s, origin)) return this.info(s);
    await this.approve({ id: newId('apr'), kind: 'connect', origin, address: account.address, network: this.networkRef(n), createdAt: this.now() });
    const byNet = (s.connections[n.id] ??= {});
    byNet[origin] = { connectedAt: this.now() };
    await this.save();
    return this.info(s);
  }

  async disconnect(origin: string): Promise<void> {
    const s = await this.st();
    if (!this.isConnected(s, origin)) return;
    delete s.connections[s.networkId]?.[origin];
    await this.save();
    this.deps.emit(origin, { event: 'disconnect', payload: { network: s.networkId } });
  }

  async getAccount(origin: string): Promise<AccountInfo | null> {
    const s = await this.st();
    if (!s.vault || s.accounts.length === 0 || !this.isConnected(s, origin)) return null;
    return this.info(s);
  }

  private async requireConnected(origin: string): Promise<{ s: WalletState; account: AccountMeta; network: Network }> {
    const s = await this.st();
    const account = this.activeAccount(s);
    if (!this.isConnected(s, origin)) throw new WalletError('NOT_CONNECTED');
    const network = this.requireSigningNetwork(s);
    return { s, account, network };
  }

  async signMessage(origin: string, params: unknown): Promise<{ address: string; signature: string }> {
    const message = typeof params === 'string' ? params : rec(params)['message'];
    if (typeof message !== 'string' || message.length === 0) throw new WalletError('INVALID_REQUEST', 'message must be a non-empty string');
    if (message.length > 16_384) throw new WalletError('INVALID_REQUEST', 'message too long');
    const { account, network } = await this.requireConnected(origin);
    await this.approve({ id: newId('apr'), kind: 'signMessage', origin, address: account.address, network: this.networkRef(network), createdAt: this.now(), message });
    const key = this.requireKey(account.index);
    return { address: account.address, signature: signMessage(key, message) };
  }

  private checkEnvelope(envelope: Envelope, account: AccountMeta, network: Network): void {
    if (envelope.chain_id !== network.chainId) throw new WalletError('WRONG_NETWORK', `Action is for chain id ${envelope.chain_id}; the wallet is on ${network.name} (chain id ${network.chainId}).`);
    if (envelope.signer !== account.address) throw new WalletError('INVALID_REQUEST', 'envelope.signer must equal the connected address');
  }

  async signAction(origin: string, params: unknown) {
    const p = rec(params);
    const envelope = validateEnvelope(p['envelope']);
    const { account, network } = await this.requireConnected(origin);
    this.checkEnvelope(envelope, account, network);
    const decoded: DecodedAction = decodeAction(envelope.action);
    const ctxRaw = rec(p['context']);
    const context = { title: typeof ctxRaw['title'] === 'string' ? ctxRaw['title'] : undefined, description: typeof ctxRaw['description'] === 'string' ? ctxRaw['description'] : undefined };
    const req: ApprovalRequest = { id: newId('apr'), kind: 'signAction', origin, address: account.address, network: this.networkRef(network), createdAt: this.now(), envelope, decoded };
    if (context.title !== undefined || context.description !== undefined) req.context = context;
    await this.approve(req);
    const key = this.requireKey(account.index);
    return signAction(key, envelope.nonce, envelope.chain_id, envelope.action);
  }

  async authorizeSession(origin: string, params: unknown) {
    const sreq = validateSessionRequest(params, Math.floor(this.now() / 1000));
    const { account, network } = await this.requireConnected(origin);
    const action = sessionAction(sreq);
    const envelope = validateEnvelope({ signer: account.address, nonce: sreq.nonce, chain_id: sreq.chain_id, action });
    this.checkEnvelope(envelope, account, network);
    const scope: SessionScope[] = sreq.scope;
    await this.approve({
      id: newId('apr'), kind: 'authorizeSession', origin, address: account.address, network: this.networkRef(network), createdAt: this.now(),
      envelope, decoded: decodeAction(action), sentence: sessionSentence(scope, sreq.expires_at, origin), scope, expires_at: sreq.expires_at, key: sreq.key,
    });
    const key = this.requireKey(account.index);
    return signAction(key, envelope.nonce, envelope.chain_id, envelope.action);
  }

  // ------------------------------------------------------------------ UI (popup)

  async uiState(): Promise<UiState> {
    const s = await this.st();
    const n = this.network(s);
    const active = s.accounts.find((a) => a.index === s.activeIndex) ?? s.accounts[0] ?? null;
    return {
      hasVault: s.vault !== null,
      locked: !this.deps.session.unlocked,
      remainingMs: this.deps.session.remainingMs,
      network: n,
      networks: NETWORKS,
      accounts: s.accounts,
      activeIndex: active?.index ?? 0,
      address: active?.address ?? null,
      connectedOrigins: Object.keys(s.connections[n.id] ?? {}),
      settings: s.settings,
      pending: this.deps.approvals.list(),
    };
  }

  /** Popup requests. Errors are surfaced as `UiError` messages. */
  async handleUi(method: string, params: unknown): Promise<unknown> {
    const p = rec(fromWire(params));
    switch (method) {
      case 'ping':
        return { ok: true, unlocked: this.deps.session.unlocked };
      case 'getState':
        return this.uiState();
      case 'generateMnemonic':
        return { mnemonic: generateMnemonic() };
      case 'validateMnemonic':
        return { valid: typeof p['mnemonic'] === 'string' && validateMnemonic(p['mnemonic']) };
      case 'createVault':
        return this.createVault(String(p['mnemonic'] ?? ''), String(p['password'] ?? ''));
      case 'unlock':
        return this.unlock(String(p['password'] ?? ''));
      case 'lock':
        this.deps.session.lock();
        return { ok: true };
      case 'touch':
        this.deps.session.touch();
        return { ok: true };
      case 'addAccount':
        return this.addAccount(typeof p['name'] === 'string' ? p['name'] : undefined);
      case 'selectAccount':
        return this.selectAccount(Number(p['index']));
      case 'renameAccount':
        return this.renameAccount(Number(p['index']), String(p['name'] ?? ''));
      case 'switchNetwork':
        return this.switchNetwork(String(p['id'] ?? ''));
      case 'disconnectOrigin':
        return this.disconnectOrigin(String(p['origin'] ?? ''));
      case 'decideApproval':
        return this.decideApproval(String(p['id'] ?? ''), Boolean(p['approved']));
      case 'setAutoLock':
        return this.setAutoLock(Number(p['minutes']));
      case 'revealMnemonic':
        return this.revealMnemonic(String(p['password'] ?? ''));
      case 'resetWallet':
        return this.resetWallet(String(p['password'] ?? ''));
      default:
        throw new UiError(`Unknown UI method ${method}`);
    }
  }

  async createVault(mnemonicRaw: string, password: string): Promise<UiState> {
    const s = await this.st();
    if (s.vault) throw new UiError('A wallet already exists. Reset it first.');
    const mnemonic = normalizeMnemonic(mnemonicRaw);
    if (!validateMnemonic(mnemonic)) throw new UiError('That is not a valid 24-word BIP39 phrase.');
    if (password.length < 8) throw new UiError('Password must be at least 8 characters.');
    const seed = mnemonicToSeed(mnemonic);
    s.vault = await encryptVault({ mnemonic }, password);
    s.accounts = [{ index: 0, address: accountAddress(seed, 0), name: 'Account 1' }];
    s.activeIndex = 0;
    await this.save();
    this.deps.session.unlock(seed, s.settings.autoLockMinutes);
    seed.fill(0);
    return this.uiState();
  }

  async unlock(password: string): Promise<UiState> {
    const s = await this.st();
    if (!s.vault) throw new UiError('No wallet yet.');
    let mnemonic: string;
    try {
      mnemonic = (await decryptVault(s.vault, password)).mnemonic;
    } catch (e) {
      if (e instanceof WrongPasswordError) throw new UiError('Wrong password.');
      throw e;
    }
    const seed = mnemonicToSeed(mnemonic);
    this.deps.session.unlock(seed, s.settings.autoLockMinutes);
    seed.fill(0);
    return this.uiState();
  }

  async addAccount(name?: string): Promise<UiState> {
    const s = await this.st();
    const seed = this.deps.session.currentSeed();
    if (!seed) throw new UiError('Unlock the wallet first.');
    const index = s.accounts.reduce((m, a) => Math.max(m, a.index), -1) + 1;
    s.accounts.push({ index, address: accountAddress(seed, index), name: name?.trim() || `Account ${index + 1}` });
    await this.save();
    this.deps.session.touch();
    return this.uiState();
  }

  async selectAccount(index: number): Promise<UiState> {
    const s = await this.st();
    if (!s.accounts.some((a) => a.index === index)) throw new UiError('No such account.');
    if (s.activeIndex !== index) {
      s.activeIndex = index;
      await this.save();
      const info = this.info(s);
      for (const origin of Object.keys(s.connections[s.networkId] ?? {})) this.deps.emit(origin, { event: 'accountChanged', payload: info });
    }
    return this.uiState();
  }

  async renameAccount(index: number, name: string): Promise<UiState> {
    const s = await this.st();
    const a = s.accounts.find((x) => x.index === index);
    if (!a) throw new UiError('No such account.');
    a.name = name.trim() || a.name;
    await this.save();
    return this.uiState();
  }

  async switchNetwork(id: string): Promise<UiState> {
    const s = await this.st();
    const n = findNetwork(id);
    if (!n) throw new UiError('Unknown network.');
    if (s.networkId !== n.id) {
      s.networkId = n.id;
      await this.save();
      const origins = new Set<string>();
      for (const byNet of Object.values(s.connections)) for (const o of Object.keys(byNet)) origins.add(o);
      for (const origin of origins) {
        this.deps.emit(origin, { event: 'networkChanged', payload: { network: n.id, chainId: n.chainId } });
        const payload = s.accounts.length > 0 && this.isConnected(s, origin) ? this.info(s) : null;
        this.deps.emit(origin, { event: 'accountChanged', payload });
      }
    }
    return this.uiState();
  }

  async disconnectOrigin(origin: string): Promise<UiState> {
    await this.disconnect(origin);
    return this.uiState();
  }

  async decideApproval(id: string, approved: boolean): Promise<UiState> {
    if (approved && !this.deps.session.unlocked) throw new UiError('Unlock the wallet first.');
    this.deps.approvals.decide(id, approved);
    return this.uiState();
  }

  async setAutoLock(minutes: number): Promise<UiState> {
    const s = await this.st();
    if (!Number.isFinite(minutes) || minutes < 1 || minutes > 24 * 60) throw new UiError('Auto-lock must be between 1 minute and 24 hours.');
    s.settings.autoLockMinutes = Math.round(minutes);
    await this.save();
    if (this.deps.session.unlocked) this.deps.session.setTtl(s.settings.autoLockMinutes);
    return this.uiState();
  }

  async revealMnemonic(password: string): Promise<{ mnemonic: string }> {
    const s = await this.st();
    if (!s.vault) throw new UiError('No wallet yet.');
    try {
      return { mnemonic: (await decryptVault(s.vault, password)).mnemonic };
    } catch (e) {
      if (e instanceof WrongPasswordError) throw new UiError('Wrong password.');
      throw e;
    }
  }

  async resetWallet(password: string): Promise<UiState> {
    const s = await this.st();
    if (s.vault) {
      try {
        await decryptVault(s.vault, password);
      } catch (e) {
        if (e instanceof WrongPasswordError) throw new UiError('Wrong password.');
        throw e;
      }
    }
    this.deps.approvals.rejectAll('USER_REJECTED');
    this.deps.session.lock();
    const origins = new Set<string>();
    for (const byNet of Object.values(s.connections)) for (const o of Object.keys(byNet)) origins.add(o);
    s.vault = null;
    s.accounts = [];
    s.activeIndex = 0;
    s.connections = {};
    await this.save();
    for (const origin of origins) this.deps.emit(origin, { event: 'disconnect', payload: { network: s.networkId } });
    return this.uiState();
  }
}
