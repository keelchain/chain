# Building a client on Keel

A client is any service that signs actions for its users or holds a role on
the chain: an exchange, a marketplace, a payments desk, a bot. This guide is
the integration contract: how a client runs its own ledger and custody next
to Keel, turns Keel on one network at a time, and reconciles everything by
transaction id. `models.md` explains the two models (custodial client with
its own vaults, P2P client on Keel Wallet); this page is the mechanics, and
it is the same for both.

## What is stable

`https://testnet.keelchain.com/rpc/v1` (node) and `/api/v1` (indexer) are
the contract. Within `v1` routes and fields are only added, never removed or
renamed; a deprecation is announced thirty days ahead in the changelog and
kept working meanwhile. The reference pages are `rpc`, `api` and `ws` under
keelchain.com/docs, and `@keelchain/sdk` on npm tracks them.

## Accounts and roles

- **One Keel account per user.** An account is an Ed25519 public key. A
  custodial client derives and stores its users' keys (or keeps a single
  aggregate account, see `models.md`); a P2P client's users hold theirs in
  Keel Wallet. Either way the client submits signed actions to `POST
  /v1/actions` and reads receipts.
- **One operator account per client**, holding the roles governance granted
  it: **attester** (it stamps a KYC tier on its users' accounts with
  `Attest`, which the chain reads for limits) and, when it runs its own
  vault, the **vault owner**. The `Onboard client` workflow gives a testnet
  client its roles and a starting balance of KEEL and KUSD; on mainnet the
  same goes through a governance proposal. `GET /v1/gov/roles` shows the
  current sets.
- **Budget, not gas.** Every account gets a free daily action budget; an
  operator account that signs at volume locks KEEL for capacity
  (`LockBudget`) or buys it (`BuyBudget`). `GET /v1/accounts/{addr}` shows
  the budget left.

## The per-network switch

Turn Keel on one external network at a time. The switch lives in the
client: when `bitcoin` is on, **new** deposit addresses, **new** withdrawals
and **new** escrow for Bitcoin go through Keel; everything that started
before stays on the old custody path until it completes; nothing is moved
between the two. The client's ledger stays the client's; Keel's balances are
the Keel side of it.

Before turning a network on, and continuously afterwards, read

```
GET /rpc/v1/ready/{BTC|ETH|TRON}
```

It answers `ready: true` only when the network's vault is registered, the
light-client checkpoint is fresh (Bitcoin), nothing is halted, and it lists
the reasons otherwise, plus the last deposit credited, the pending outbounds
and the fee rate. A client flips its switch off when `ready` goes false and
back on when it recovers; no funds are at risk in between, only new
operations wait.

Checklist for one network:

1. `ready` is true.
2. The operator account has budget (locked or bought KEEL).
3. One deposit address issued for a test account (`RequestDepositAddress`),
   and a small deposit credited (`deposits:<chain>:<addr>` on the socket, or
   `GET /v1/vaults/deposits?owner=`).
4. One small withdrawal to an address the client controls, confirmed
   (`WithdrawalQueued` → `OutboundBatched` → `OutboundConfirmed`).
5. For a P2P client: one escrow round trip (`StartTrade`, `MarkPaid`,
   `ReleaseTrade`).
6. The reconciliation job below has run clean for a day.

## Custodial clients: your own vault

A client that keeps its own keys (docs/models.md, Model A) uses the custody
actions instead of the network vault: `RegisterCustodyVault` once per chain
(public key, chain code, signer URL), `RequestCustodyAddress` per account,
`WithdrawCustody` per payout. The observers attest deposits on the client's
addresses like any other; the client's signer signs its batches after the
same policy check the network signers run. Balances land in the `custody`
account type, separate from network-backed balances. `GET /v1/custody/{addr}`
is the client's reserve versus liabilities per asset; the per-network switch
applies the same way (enable a network once its custody vault is registered
and `/v1/ready/{chain}` is green for deposits).

## Idempotency and reconciliation

Every signed action carries the signer's **nonce**; the chain applies each
nonce once and in order, so the nonce is the idempotency key. A client keeps
a table

```
client_id -> (signer, nonce, tx_id, status)
```

and computes `tx_id` locally before submitting (`txId(signedAction)` in the
SDK, the hash of the signed envelope), so a crashed process can look the
receipt up (`GET /v1/receipts/{tx_id}`, or the indexer's `/v1/txs/{tx_id}`
for anything older than the node's recent window) instead of signing again.
A duplicate submit of the same signed bytes is rejected as a nonce reuse and
is harmless. Free-text `memo` fields on transfers are for people, not for
matching.

Reconciliation: once a day, for every Keel account the client manages,
compare `GET /v1/accounts/{addr}` balances (deposit, escrow, sendout
escrow) with the client's ledger, and every outbound of the day with the
indexer's `/v1/vaults/outbounds`. The explorer shows the same numbers to the
client's users.

## Streams

Subscribe rather than poll for anything time-sensitive: `account:<addr>`
for a user's receipts, `deposits:<chain>:<addr>` for credits, `pair:` and
`book:` for markets. Every frame has a sequence number and the server tells
a client when it fell behind (`gap`); the SDK's `KeelSocket` reconnects and
replays from the last height. Details in the `ws` reference.

## Signing

- Custodial client: sign server-side with `@keelchain/sdk` (`Keypair`,
  `RpcClient.send`), keys in the client's KMS.
- P2P client: ask the user's Keel Wallet (`window.keel.signAction`) and
  submit what comes back; use a **session key** (`AuthorizeSessionKey`, scope
  `markets` or `p2p_manage`) so trading and offer management do not need a
  popup per click. Session keys can never move funds. `wallet.md` has the
  provider contract.
- Withdrawals from a client-owned vault are signed by the client's own
  signer under the same policy every Keel signer runs: the transaction must
  pay open outbounds of a finalized batch.

## Pricing

Everything Keel charges is paid in KEEL, bought on the chain's own order
book with KUSD, USDT, BTC or TRX; there is no invoice.

- **Custodial clients** pay usage: per deposit address issued in their
  vault, per outbound batch built and broadcast, per network watched, plus
  a service tier chosen by locking KEEL. Nothing is a share of the client's
  own fees.
- **P2P clients** pay a wholesale protocol fee on escrow releases,
  order-book fills and withdrawals, and set their own retail fee on top
  (`SetClientFee`), which the chain pays into the client's account in the
  same posting.
- **Service tiers** come from locked KEEL on the client's operator account:
  higher limits, webhooks and a revenue dashboard. `tokenomics.md` has the
  schedule and the governance parameters behind it.

## Testnet policy

The testnet resets when the genesis has to change (a validator set change,
a schema change that cannot migrate). Resets are announced in the changelog
and re-onboard every client listed with the workflow; balances are testnet
balances. `GET /rpc/v1/status` carries the chain id and the state hash a
client can pin.

## Getting help

mohab@keelchain.com for roles, API access and partnerships;
support@keelchain.com for everything else.
