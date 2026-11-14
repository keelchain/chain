# Funding an account

Keelchain accounts hold assets from other chains through **vaults**: a
threshold-signed key per chain whose child addresses are handed out to
accounts. You send coins to your child address on the external chain, the
observers watch it, and after that chain's confirmation depth the amount is
credited to your Keelchain account as a vault asset (`BTC.BTC`, `TRON.USDT`,
`TRON.TRX`, `BSC.USDT`, `BSC.BNB`, `SOL.SOL`, `SOL.USDC`, `ETH.ETH`,
`ETH.USDT`). `KEEL` and `KUSD` never leave the chain: `KUSD` is minted only
against vaulted USDT, `KEEL` is transferred.

On the public testnet the external chains are **Bitcoin signet**, **Tron
Nile**, the **BNB Smart Chain testnet** and **Solana devnet**. Mainnet coins
sent to a testnet address are lost.

BNB Smart Chain and Ethereum share one address: the chain derives both from
the same key path, so the `0x…` string you get for BSC is the one you would
get for Ethereum, and a deposit index assigned on one is the other's too.

## 1. Get your deposit address

The chain assigns the address; it is not derived in the wallet. Three ways:

**Keel Wallet.** Open the wallet, card *Receive*, press *Bitcoin*, *Tron*,
*BNB Smart Chain* or *Solana*.
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
- **BNB Smart Chain testnet:** get test BNB from the BNB Chain faucet and
  USDT (the testnet BEP-20 at `0x3376…4dDd`) and send to the `0x…` address.
  Credited after **12 confirmations** (`confirmations_eth`, shared with
  Ethereum), about forty seconds. BNB is credited as `BSC.BNB` from the
  transaction's own transfer: coins sent by a contract (an internal
  transfer) are not seen.
- **Solana devnet:** get devnet SOL from the Solana faucet (`solana airdrop`
  or faucet.solana.com) and devnet USDC (mint `4zMM…ncDU`, from the Circle
  faucet) and send to the base58 address. Credited once the transaction is
  **finalized** (the observers read finalized transactions only; the chain's
  own depth check uses `confirmations_tron` slots). USDC lands in the
  address's associated token account, which the sending wallet creates.

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
addresses live on Bitcoin **signet**, Tron **Nile**, the **BSC testnet**
(chain id 97) and Solana **devnet**. Coins sent on testnet3, testnet4,
Shasta, Ethereum, Solana mainnet or any other mainnet to the same address
string are on a different ledger, the observers never see them, and they
cannot be credited.

## Sent on the wrong network

Every Keel deposit address is a child of the vault key, and the chain
records which account owns it. Anything that ever lands at that address,
on any network, in any token, is that account's, even when the chain
cannot credit it: USDT sent on Polygon or Arbitrum to a BSC address (the
EVM family shares one address), a token the chain does not list, SOL sent
on mainnet to a devnet address. Recovery is self-service and automatic:

1. In Keel Wallet, *Deposits* → *Sent on the wrong network?*: choose the
   address family, the EVM chain id when the coins are on another EVM
   network, the token contract or mint (empty for the native coin), and an
   address of yours on that network. The wallet signs a `RequestRecovery`
   and pays `recovery.fee_keel` (0 on the testnet).
2. The chain checks the address is yours and opens a recovery row; nobody
   approves anything. A row nobody can serve expires after
   `recovery.expiry_blocks`.
3. The observers move what sits at the address to your destination,
   signed under the same policy as payouts (the signer accepts only a
   transfer out of that index, to that destination, in that token), and
   report it. The row shows `Done` with the amount and the transaction.

Gas: on a network the observers serve, the vault's hot address funds the
transfer's gas as it does for consolidation. On another EVM network, send a
little of that network's native coin to the same address first; the
transfer pays its own gas from it. A custodial client asks for its users'
addresses with `custodian` and `owner` set; its own signer signs.

What stays impossible: coins sent to a Bitcoin address on another chain
(no such transaction can exist), and anything whose address format differs.

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
