# Funding an account

Keelchain accounts hold assets from other chains through **vaults**: a
threshold-signed key per chain whose child addresses are handed out to
accounts. You send coins to your child address on the external chain, the
observers watch it, and after that chain's confirmation depth the amount is
credited to your Keelchain account as a vault asset (`BTC.BTC`, `TRON.TRX`,
`BSC.BNB`, `SOL.SOL`, `SOL.USDC`, `ETH.ETH`). **USDT is the exception: USDT
sent on Ethereum, Tron or BNB Smart Chain is credited as one `USDT`
balance**, backed one to one by the USDT the vaults hold on every network,
and withdrawn as USDT on the network you choose ("Withdrawing" below).
`KEEL` never leaves the chain.

On the public testnet the external chains are **Bitcoin signet**, **Tron
Nile**, the **BNB Smart Chain testnet** and **Solana devnet**. Mainnet coins
sent to a testnet address are lost.

BNB Smart Chain and Ethereum share one address: the chain derives both from
the same key path, so the `0x…` string you get for BSC is the one you would
get for Ethereum, and a deposit index assigned on one is the other's too.

## 0. Lock one KEEL first

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

Your users lock nothing. An account you attested acts on your plan's daily
pool of actions while the plan runs (`clients.md`, Pricing), so neither
the request for a custody address nor a withdrawal from your vault needs
KEEL on the user's account, whether you sign it as custodian or the user
signs it in Keel Wallet. What has to be locked is your operator account's
tier KEEL, and that is the plan itself (`exchange.md`). Without a running
plan your users are addresses like any other: the *Wallet user* tab
applies to them, and you send each one the KEEL for its lock with its
attestation.

</div>
<div class="tab" data-mode="noncustodial">

Your users lock nothing while your plan runs: an account you attested acts
on your plan's daily pool of actions (up to 100 a day per user,
`clients.md`, Pricing), and the wallet's *Action capacity* card tells them
so instead of asking for a lock. A user's own locked KEEL, if any, is used
first. Your operator account keeps its tier's KEEL locked; that lock is
one half of the plan. Without a running plan your users pay their own way,
as in the *Wallet user* tab, and you send each one the KEEL for its lock
with its attestation.

</div>
<div class="tab" data-mode="user">

An address acts on the capacity of the KEEL it has locked; there is no
free action budget and no fee per action (`governance.md`). The request
for a deposit address is an action like any other, so do this first: get
at least 1 KEEL (on the testnet, `faucet.keelchain.com` sends 100 KEEL and
100 USDT to any address; a client's user gets it from the client with its
attestation; anyone else receives a transfer), then lock 1 KEEL from the
wallet's *Action capacity* card. That is a hundred actions a day, and the
KEEL comes back in full three days after you unlock it. Until then the
chain answers `BUDGET_EXHAUSTED` (or `NOT_ACTIVATED` to an address holding
less than 0.1 KEEL), and the wallet says so.

</div>
</div>

## 1. Get your deposit address

The chain assigns the address; it is not derived in the wallet. Three ways:

**Keel Wallet.** Open the wallet, card *Receive*, press *Bitcoin*, *Tron*,
*BNB Smart Chain* or *Solana*.
The wallet asks the chain for an address the first time (a signed action
on your locked capacity, no site involved) and shows it with a copy button from
then on.

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

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

Your users' addresses are in **your vault**, not the network's. Once the
vault for the chain is registered (`RegisterCustodyVault`), request an
address with `RequestCustodyAddress { chain, custodian }`: you sign it as
custodian for an account in your custody, or the user signs it in Keel
Wallet, where *Receive* shows the address labelled as your custody
(`wallet.md` §5c). Index 0 is the vault's own address for your gas and
float top-ups. Read the addresses and their owners at
`GET /v1/custody/{you}/{chain}/addresses?owner=`, at deposit time rather
than from a cache: a vault rotation keeps every account's index and
changes the string derived from it (`models.md`, "Rotating to a fresh
pool"). A user's deposit is credited to the `custody` account type, backed
by your vault's reserve, and `GET /v1/custody/{you}` shows that reserve
against the balances it backs. Each address issued is usage
(`clients.usage_address_usd_micro`, $0.02), drawn from your plan's
allowance first.

</div>
<div class="tab" data-mode="noncustodial">

Your users' addresses are in the **network vault**, exactly as above. The
user's own wallet signs the `RequestDepositAddress` (from the Receive
card, or from a signing request your site raises through
`window.keel.signAction`), and your site shows the address it reads back
from `GET /v1/vaults/{chain}/addresses?owner=<account>`; a session key
cannot request one. The action comes out of your plan's pool, and the
address issued is usage (`clients.usage_address_usd_micro`, $0.02),
drawn from your plan's allowance first; an address you cannot pay for is
refused until the usage owed is settled.

</div>
<div class="tab" data-mode="user">

The three ways above are yours. The chain hands you a child address of
the network vault's key on every network it has a vault for, one index per
account per chain, and you pay nothing for it beyond the action itself.

</div>
</div>

<div class="tabs" data-group="wallet">
<div class="tab" data-mode="keel">

Everything on this page is about coins **in Keel**: an address the chain
assigned, a balance the observer set signs for, a credit that trades,
escrows and swaps.

</div>
<div class="tab" data-mode="wallet">

Coins can also stay **in your wallet**, under your own key. When
*Settings → New deposits go to* says *your wallet*, the Receive card
shows instead a Bitcoin or Tron address the wallet derived from your seed
on the standard paths (`wallet.md` §5b). Nothing is asked of the chain:
no action, no capacity, no index, and the chain never learns the address.
Coins there do not trade until you move them into Keel (*Wallet → Move to
Keel*), which is an ordinary deposit to the address this section
describes.

</div>
</div>

## 2. Send coins to it

- **Bitcoin signet:** get coins from a signet faucet (search "bitcoin signet
  faucet"; the Bitcoin Core project lists current ones) and send them to the
  `tb1…` address. Credited after **2 confirmations** (`confirmations_btc`),
  about 20 minutes on signet.
- **KEEL and USDT:** `faucet.keelchain.com` sends both to any account address.
- **Tron Nile:** get test TRX and USDT from the Nile faucet
  (nileex.io) and send to the `T…` address. Credited after **19
  confirmations**, about one minute. The USDT is credited as `USDT`, your
  one dollar balance, not as a Tron-specific asset (`policy`).
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

<div class="tabs" data-group="wallet">
<div class="tab" data-mode="keel">

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

</div>
<div class="tab" data-mode="wallet">

Coins sent to the address in your wallet never reach the chain, so there
are no stages: the popup's *Wallet* tab reads the address's balance from
mempool.space or TronGrid directly and shows it the moment the network
does. The explorer shows nothing, since the chain never learns the
address, and no observer set stands behind it. To trade, move the coins
into Keel (*Wallet → Move to Keel*); from then on they are an ordinary
deposit and follow the stages in the *In Keel* tab. `wallet.md` §5b.

</div>
</div>

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
transfer pays its own gas from it.

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

For an address in your vault you make the request yourself:
`RequestRecovery` with `custodian` and `owner` set, signed by you as
custodian or by the user in Keel Wallet, and your own signer signs the
transfer under the same policy. The destination is the user's address on
that network, and the row is visible on your custody page like any other.

</div>
<div class="tab" data-mode="noncustodial">

Your users do the steps above from Keel Wallet themselves, or your site
builds the `RequestRecovery` and asks the wallet to sign it; a session key
cannot. The address must be the account's own, and the destination is
the user's; your service never holds the coins at any point.

</div>
<div class="tab" data-mode="user">

The steps above are the whole procedure: your wallet, your address, your
destination, nobody to ask.

</div>
</div>

What stays impossible: coins sent to a Bitcoin address on another chain
(no such transaction can exist), and anything whose address format differs.

## What has to be true on the chain side

- A vault is registered for the chain (`/v1/vaults/BTC` is not null). The
  deploy registers the BTC, TRON, BSC and SOL vaults on a fresh testnet.
- For Bitcoin, a **checkpoint** exists: deposits are verified by a light
  client from a governance-set header, so a brand-new chain credits nothing
  until the first `SetBtcCheckpoint` proposal executes. The deploy proposes
  one right after bootstrapping (about six minutes of voting and timelock) and
  a timer proposes a fresh one daily. `/v1/vaults/BTC` shows the checkpoint
  height.
- An observer is running with access to a Bitcoin node, TronGrid, a BNB
  Smart Chain RPC and a Solana RPC; it builds the deposit observations and,
  with the quorum, signs payouts.

## Withdrawing

<div class="tabs" data-group="client">
<div class="tab" data-mode="custodial">

A `custody` balance leaves through **your vault**, never through the
network's: `WithdrawCustody` (signed by you as custodian, or by the user
from Keel Wallet's *Send → from custody*) escrows the custody balance, the
chain batches your vault's outbounds separately from the network's, an
observer builds the transaction, your own signer signs it after the policy
check, and the observers broadcast and confirm it. Network fees come out of
your vault's index-0 gas balance when it has one, otherwise they are booked
as your expense until the next top-up. Each withdrawal is usage
(`clients.usage_outbound_usd_micro`, $0.10). A balance that has been swapped
to the `deposit` side (`SwapCustody`) is withdrawn as in the other tabs, or
swapped back first. The whole procedure is in `models.md`, Model A.

</div>
<div class="tab" data-mode="noncustodial">

Your users withdraw as in the *Wallet user* tab, with their own signature:
from Keel Wallet's *Send*, or from a signing request your site raises;
`Withdraw` and `WithdrawStable` are never in a session key's scope. While
your plan runs the flat withdrawal fee is waived for them and you pay
usage instead (`clients.usage_outbound_usd_micro`, $0.10), drawn from
your allowance first; a withdrawal you cannot pay for is refused until the
usage owed is settled.

</div>
<div class="tab" data-mode="user">

`Withdraw { asset, to, amount }` (wallet: *Send*, CLI: `keel send withdraw
BTC.BTC <address> <sats>`) queues a payout; the observers batch, sign with the
vault key and broadcast. The flat withdrawal fee comes from
`withdraw_flat_fee_usd_micro`.

**USDT** is withdrawn with `WithdrawStable { via, to, amount }` (wallet:
*Send*, asset USDT, then pick the network): `via` is the network's USDT to
pay out in (`TRON.USDT`, `BSC.USDT`, `ETH.USDT`), `amount` is in USDT. The
chain redeems the balance into that network's reserve and queues the payout
in one step; you never hold the per-network asset. A network can pay out
only what its reserve holds, so pick one that has it (`GET /v1/stable`
lists each network's reserve). A network below its target share of the
reserve charges a premium of up to `stable.rebalance_max_premium_bps` (1%),
scaled by the shortfall, which the wallet shows before you confirm; a
deposit of USDT on that network earns the same rate back as a rebate. That
premium is how the reserve rebalances without anyone at Keel moving coins
(`how-it-works.md` §3.2, keelchain.com/policy).

</div>
</div>

<div class="tabs" data-group="wallet">
<div class="tab" data-mode="keel">

The withdrawals above move coins out of Keel. *Send → Move to your wallet*
is the same withdrawal with the destination locked to your own address for
that chain (`wallet.md` §5b).

</div>
<div class="tab" data-mode="wallet">

Coins in your wallet are already outside Keel, so there is no withdrawal
to make and no fee, batch or observer involved. The extension itself only
shows them and moves them into Keel; to send them anywhere else, import
the same phrase into any Bitcoin or Tron wallet, which derives the same
keys on the standard paths.

</div>
</div>
