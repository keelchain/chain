# Funding an account

Keelchain accounts hold assets from other chains through **vaults**: a
threshold-signed key per chain whose child addresses are handed out to
accounts. You send coins to your child address on the external chain, the
observers watch it, and after that chain's confirmation depth the amount is
credited to your Keelchain account as a vault asset (`BTC.BTC`, `TRON.USDT`,
`TRON.TRX`, `ETH.ETH`, `ETH.USDT`). `KEEL` and `KUSD` never leave the chain:
`KUSD` is minted only against vaulted USDT, `KEEL` is transferred.

On the public testnet the external chains are **Bitcoin signet** and **Tron
Nile**. Mainnet coins sent to a testnet address are lost.

## 1. Get your deposit address

The chain assigns the address; it is not derived in the wallet. Three ways:

**Keel Wallet.** Open the wallet, card *Receive*, press *Bitcoin* or *Tron*.
The wallet asks the chain for an address the first time (a free signed
action, no site involved) and shows it with a copy button from then on.

**CLI.**

```bash
keel --rpc https://testnet.keelchain.com/rpc --chain-id 3 send --secret <secret> deposit-address BTC
keel --rpc https://testnet.keelchain.com/rpc --chain-id 3 deposit-address BTC <your address>
```

**SDK.**

```ts
const rpc = new RpcClient("https://testnet.keelchain.com/rpc");
const { index, address } = await rpc.requestDepositAddress(key, "BTC", 3);
```

Under the hood: the account signs `RequestDepositAddress { chain }`; the chain
records the next free index for that account and chain; anyone can read the
address at `/v1/vaults/BTC/addresses?owner=<account>` or
`/v1/vaults/BTC/addresses/<index>`. One index per account per chain; asking
again returns the same one. A custodial client's users keep their index
across the client's vault rotations; only the address derived from it
changes (`models.md`, "Rotating to a fresh pool").

## 2. Send coins to it

- **Bitcoin signet:** get coins from a signet faucet (search "bitcoin signet
  faucet"; the Bitcoin Core project lists current ones) and send them to the
  `tb1…` address. Credited after **2 confirmations** (`confirmations_btc`),
  about 20 minutes on signet.
- **KEEL and KUSD:** `faucet.keelchain.com` sends both to any account address.
- **Tron Nile:** get test TRX and USDT from the Nile faucet
  (nileex.io) and send to the `T…` address. Credited after **19
  confirmations**, about one minute.

Any amount works. Deposits above `large_deposit_usd_micro` (50,000 USD
equivalent) are held for `large_deposit_delay_blocks` before they are usable.

## 3. See it land

The chain learns about a deposit only when an observer reports it at the
required depth, so for the first minutes the chain has nothing to show. The
wallet and the explorer fill that gap by looking the deposit address up on
the external network's public API (mempool.space for Bitcoin, TronGrid for
Tron) straight from the browser, and merge what they see with the chain's
own rows. Each incoming transaction moves through these stages:

| Stage | Meaning |
|---|---|
| seen | in the external network's mempool, no confirmation yet |
| confirming | confirmed, below the required depth (`1 of 2 confirmations`) |
| observing | deep enough; the observers' quorum is being collected |
| held | credited, but delayed as a large deposit until a release height |
| credited | in your balance |
| rejected | the observers disagreed; nothing credited |

- Wallet: the *Deposits* card on the home screen lists them with a
  `processing` count, refreshing every 30 seconds while the popup is open.
- Explorer: your account page → *Deposits* tab, one card per chain with the
  address, the network it lives on and the stages above.
- RPC: `/v1/accounts/<account>` lists balances; `/v1/vaults/deposits?owner=<account>`
  lists pending and credited deposits with the external transaction hash.
- SDK: `externalNetworks`, `fetchIncoming` and `depositProgress` are the same
  helpers the wallet and the explorer use.

Coins can also stay **in your wallet**, under your own key, and be moved
into Keel when you want to trade: `wallet.md` §5b.

If nothing appears after a few minutes, check the network: the testnet
addresses live on Bitcoin **signet** and Tron **Nile**. Coins sent on
testnet3, testnet4, Shasta or a mainnet to the same address string are on a
different ledger, the observers never see them, and they cannot be credited.

## What has to be true on the chain side

- A vault is registered for the chain (`/v1/vaults/BTC` is not null). The
  deploy registers BTC and TRON vaults on a fresh testnet.
- For Bitcoin, a **checkpoint** exists: deposits are verified by a light
  client from a governance-set header, so a brand-new chain credits nothing
  until the first `SetBtcCheckpoint` proposal executes. The deploy proposes
  one right after bootstrapping (about six minutes of voting and timelock) and
  a timer proposes a fresh one daily. `/v1/vaults/BTC` shows the checkpoint
  height.
- An observer is running with access to a Bitcoin node and TronGrid; it
  builds the deposit observations and, with the quorum, signs payouts.

## Withdrawing

`Withdraw { asset, to, amount }` (wallet: *Send*, CLI: `keel send withdraw
BTC.BTC <address> <sats>`) queues a payout; the observers batch, sign with the
vault key and broadcast. The flat withdrawal fee comes from
`withdraw_flat_fee_usd_micro`.
