// Connect → balances → sign a transfer → live account channel.
import { KeelSocket, RpcClient, mountConnectButton, type AccountInfo, type KeelProvider } from '@keelchain/sdk';

const RPC = 'https://testnet.keelchain.com/rpc';
const rpc = new RpcClient(RPC);
const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let provider: KeelProvider | null = null;
let account: AccountInfo | null = null;
let socket: KeelSocket | null = null;

async function showBalances(address: string): Promise<void> {
  const a = await rpc.account(address);
  const rows = (a.balances as { asset: string; account_type: string; balance: string }[]).filter((b) => b.account_type === 'deposit');
  $('balances').innerHTML = rows.length === 0
    ? '<tr><td class="muted">No balances yet. Ask for testnet KEEL at mohab@keelchain.com or receive a signet deposit from the wallet.</td></tr>'
    : rows.map((b) => `<tr><td>${b.asset}</td><td style="text-align:right">${b.balance}</td></tr>`).join('');
}

function follow(address: string): void {
  socket?.close();
  socket = new KeelSocket(rpc.wsUrl());
  const feed = $<HTMLPreElement>('feed');
  const log = (line: string) => { feed.textContent = `${line}\n${feed.textContent ?? ''}`.slice(0, 6000); };
  socket.on('subscribed', (f) => log(`subscribed at height ${f.height}: ${JSON.stringify(f.channels)}`));
  socket.on('event', (f) => { log(`#${f.seq} h${f.height} ${JSON.stringify(f.data)}`); void showBalances(address); });
  socket.on('gap', (f) => log(`gap: missed ${f.missed} (${f.from_height}..${f.to_height}); reconcile from /v1/blocks/{h}/receipts`));
  socket.on('heartbeat', (f) => log(`heartbeat h${f.height}`));
  void socket.subscribe([`account:${address}`]);
}

mountConnectButton($('connect'), {
  installUrl: 'https://keelchain.com/wallet/',
  onConnected: (a, p) => {
    provider = p;
    account = a;
    $('account').textContent = `${a.address} on ${a.network} (chain ${a.chainId})`;
    $<HTMLButtonElement>('send').disabled = false;
    void showBalances(a.address);
    follow(a.address);
  },
  onError: (e) => { $('account').textContent = e.message; $('account').className = 'err'; },
});

$('send').addEventListener('click', async () => {
  if (!provider || !account) return;
  const out = $('sendout');
  try {
    const to = $<HTMLInputElement>('to').value.trim().toLowerCase();
    const whole = $<HTMLInputElement>('amount').value.trim();
    if (!/^[0-9a-f]{64}$/.test(to)) throw new Error('the recipient is a 64-hex Keel address');
    const amount = BigInt(Math.round(Number(whole) * 1_000_000));
    const acct = await rpc.account(account.address);
    out.textContent = 'waiting for the wallet…';
    const { signed, tx_id } = await provider.signAction({
      envelope: { signer: account.address, nonce: Number(acct.nonce), chain_id: account.chainId, action: { Transfer: { to, asset: 'KEEL', amount, memo: 'try keelchain.com' } } },
      context: { title: 'Try Keel Wallet', description: `Send ${whole} KEEL` },
    });
    await rpc.submit(signed as never);
    out.innerHTML = `submitted <code>${tx_id}</code>; waiting for the receipt…`;
    const r = await rpc.waitReceipt(tx_id, 30_000, 300, 'https://testnet.keelchain.com/api');
    out.innerHTML = r.ok ? `done: <a href="https://testnet.keelchain.com/testnet/tx/${tx_id}" target="_blank" rel="noreferrer">${tx_id}</a>` : `rejected: ${JSON.stringify(r.error)}`;
    void showBalances(account.address);
  } catch (e) {
    out.textContent = e instanceof Error ? e.message : String(e);
  }
});
