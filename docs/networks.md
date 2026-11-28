# Supported networks

Keel holds coins from other chains through **vaults**: one threshold-signed
key per network, whose child addresses the chain hands to accounts. A
deposit to such an address is watched by the observers and, at the
network's confirmation depth, credited as a vault asset named
`<network>.<coin>`; a withdrawal is a batched outbound the observers sign
under the vault key. USDT is the exception: on every network it is
credited as the one `USDT` balance ("USDT, one balance" below).

Figures in a shaded box are read from the public testnet node and read API
when you open this page (`/rpc/v1/ready/{chain}`, `/rpc/v1/params`,
`/rpc/v1/stable`, `/rpc/v1/lightning`, `/api/v1/assets`, `/api/v1/vaults`),
so they show the chain as it is now. Numbers in the text that are
governance parameters are refreshed from the same call; the rest is from
the code. Mainnet has its own set of vaults; this page reads the testnet's.

<style>
.live { border: 1px solid var(--line); border-left: 3px solid var(--teal-dim); background: var(--card); border-radius: 0 12px 12px 0; padding: 12px 16px; margin: 14px 0; }
.live table { margin: 6px 0; }
.live .asof { color: var(--muted); font-size: .82rem; margin: 0 0 6px; }
.live .err { color: var(--gold); }
.ok { color: var(--teal); font-weight: 600; }
.bad { color: var(--gold); font-weight: 600; }
.doc td.num, .doc th.num { text-align: right; font-variant-numeric: tabular-nums; }
</style>

## At a glance

<div class="live" data-live="overview">
<p class="muted">Live table: needs JavaScript and the testnet node. Without it, read <code>/rpc/v1/ready/{chain}</code> for each of <code>BTC</code>, <code>ETH</code>, <code>TRON</code>, <code>BSC</code>, <code>SOL</code>.</p>
</div>

| Network | On the testnet | On mainnet | Address | Vault key | Assets |
|---|---|---|---|---|---|
| Bitcoin | signet | Bitcoin | `tb1q…` / `bc1q…` (P2WPKH) | secp256k1, CGGMP21 | `BTC.BTC`, Lightning |
| Ethereum | Sepolia | Ethereum | `0x…` (EIP-55) | secp256k1, CGGMP21, shared with BSC | `ETH.ETH`, `ETH.USDT` → `USDT` |
| BNB Smart Chain | testnet, chain id 97 | chain id 56 | `0x…`, the same string as Ethereum | shared with Ethereum | `BSC.BNB`, `BSC.USDT` → `USDT` |
| Tron | Nile | Tron | `T…` (base58check) | secp256k1, CGGMP21 | `TRON.TRX`, `TRON.USDT` → `USDT` |
| Solana | devnet | mainnet-beta | base58 Ed25519 key | Ed25519, FROST | `SOL.SOL`, `SOL.USDC` |

Everything the networks have in common (addresses, depth, large deposits,
batching, halts, recovery, the per-network switch) is in "Same on every
network" at the end. `KEEL` never leaves the chain.

## Assets

An asset is `<network>.<coin>` (`BTC.BTC`, `TRON.TRX`, `SOL.USDC`), with
the decimals of the coin on its own network, so a balance is an integer in
the smallest unit and never rounds. `KEEL` (6 decimals, the orca) is
native; `USDT` (6 decimals) is the unified dollar, backed by the per-network
USDT the vaults hold.

<div class="live" data-live="assets">
<p class="muted">Live table: every asset the chain knows, its network, decimals, supply in Keel and holders (<code>/api/v1/assets</code>).</p>
</div>

## USDT, one balance on three networks

USDT sent on Ethereum, Tron or BNB Smart Chain is credited as one `USDT`
balance, and withdrawn with `WithdrawStable { via, to, amount }` on the
network you choose. Each network's USDT is a **basket** entry with a
reserve (what the vault holds there), a target share, and a cap. A network
below its target charges a premium on the way out, up to
<span data-param="stable.rebalance_max_premium_bps" data-unit="bps">1%</span>,
scaled by the shortfall; a deposit on that network earns the same rate back
as a rebate. That is how the reserve rebalances without anyone moving coins
([how it works](../how-it-works/#32-usdt-one-balance-on-every-network),
[policy](../../policy/)). `BSC.USDT` has 18 decimals on its own chain and
`ETH.USDT` and `TRON.USDT` have 6; the unified balance is always 6.

<div class="live" data-live="stable">
<p class="muted">Live table: the basket per network, reserve, target share, premium now (<code>/rpc/v1/stable</code>).</p>
</div>

## Bitcoin

- **Networks.** Signet on the testnet (not testnet3 or testnet4); Bitcoin
  on mainnet. Addresses are native segwit P2WPKH, `tb1q…` on signet and
  `bc1q…` on mainnet.
- **Derivation.** BIP32 non-hardened child of the vault key at
  `m/0/<deposit index>` (SLIP-44 coin 0). Anyone can recompute every
  address from the registered public key and chain code; no whole key
  exists anywhere.
- **Verified by a light client** (Bitcoin and Ethereum are the two
  networks with one). Each deposit observation carries the block headers
  and a Merkle proof, the chain checks proof of work, linkage and depth
  itself, and its view of Bitcoin starts from a **checkpoint** set by
  governance (`SetBtcCheckpoint`). A fresh chain credits nothing until
  the first checkpoint executes; the testnet proposes a fresh one daily.
  The checkpoint's height and age are in the box below.
- **Depth.** Credited after
  <span data-param="confirmations_btc">2</span> confirmations
  (`confirmations_btc`), about twenty minutes on signet.
- **Withdrawals.** Batched P2WPKH spends from the vault's child keys, one
  batch every <span data-param="outbound_batch_interval_blocks">20</span>
  blocks; the fee rate below is in sat/vB as the observers estimate it.
- **Assets.** `BTC.BTC`, 8 decimals (satoshi).
- **Test coins.** Any signet faucet; the Bitcoin Core project lists current
  ones. Explorer: [mempool.space/signet](https://mempool.space/signet).

<div class="live" data-live="chain:BTC"></div>

### Lightning

Lightning needs a hot node with channels, which a chain cannot run, so an
**observer** may run an LND node and register it (`RegisterLightningNode`).
The chain lends that node BTC from the vault (`FundLightningPool`, a normal
outbound owned by the system) and keeps one **pool** per observer: what it
lent, what is pending in outgoing payments, what the node credited today.
The sum of the pools is a reserve account under the BTC vault, so the
reserve check still balances.

- **Deposit.** The payer pays a BOLT11 invoice from the observer's node
  whose description binds the owner's Keel address. On settlement the
  observer submits the invoice and preimage; every validator re-parses the
  invoice, checks the payee is that observer's node and the preimage
  matches the payment hash, and credits `BTC.BTC`. One credit per payment
  hash, so a replay is impossible.
- **Payout.** A `Withdraw` of `BTC.BTC` whose destination is a BOLT11
  invoice. The invoice must carry the amount and be unexpired; the user's
  escrow holds the amount plus a routing allowance of
  `max(amount × lightning_max_fee_bps / 10000, lightning_min_fee_sats)`,
  the payout is assigned to the registered observer with the most free pool
  balance, and whatever allowance is unspent comes back on chain. An
  observer that fails or misses the deadline triggers a full refund.
- **Sweep.** A node holding more than its pool should announces a sweep
  transaction to the vault's own address first; the deposit hook recognises
  it and books the BTC back as reserves. An unannounced deposit to the
  vault's own address is refused.
- **Hot exposure** per observer is bounded by the pool cap and backed by
  the observer's bond (`min_observer_bond`).

The parameters are governance parameters like every other
([governance](../governance/)):

| Key | Now | Meaning |
|---|---|---|
| `lightning_enabled` | <span data-param="lightning_enabled" data-unit="flag">on</span> | Off: invoices refused for deposits and payouts; on-chain BTC still works |
| `lightning_pool_cap_sats` | <span data-param="lightning_pool_cap_sats" data-unit="sats">0.5 BTC</span> | Most BTC one observer's node may hold for the chain |
| `lightning_max_deposit_sats` | <span data-param="lightning_max_deposit_sats" data-unit="sats">0.1 BTC</span> | Largest single Lightning deposit |
| `lightning_max_withdraw_sats` | <span data-param="lightning_max_withdraw_sats" data-unit="sats">0.1 BTC</span> | Largest single Lightning payout |
| `lightning_max_fee_bps` | <span data-param="lightning_max_fee_bps" data-unit="bps">0.5%</span> | Routing fee allowance, in bps of the amount |
| `lightning_min_fee_sats` | <span data-param="lightning_min_fee_sats">10</span> sats | Allowance floor for small payouts |
| `lightning_payout_timeout_blocks` | <span data-param="lightning_payout_timeout_blocks">600</span> blocks | What an observer has to pay before the chain refunds |
| `lightning_daily_cap_sats` | <span data-param="lightning_daily_cap_sats" data-unit="sats-or-none">none</span> | Total deposits one observer may credit per day |

<div class="live" data-live="lightning">
<p class="muted">Live: registered pools, open payouts and pending sweeps (<code>/rpc/v1/lightning</code>).</p>
</div>

Running a node for the chain is an observer task: the `[lightning]` section
of the observer's configuration, `keel send lightning register | fund |
sweep`, and `infra/dev/lightning-e2e.sh` for a regtest rehearsal, in the
[observer README](https://github.com/keelchain/chain/blob/main/chain/crates/keel-observer/README.md#lightning).
Invoices are pasted or scanned; there is no LNURL or Lightning Address yet.

## Ethereum

- **Networks.** Sepolia (chain id 11155111) on the testnet; Ethereum
  (chain id 1) on mainnet. Addresses are EIP-55 checksummed `0x…`.
- **Derivation.** BIP32 child at `m/60/<deposit index>` (SLIP-44 coin
  60). BNB Smart Chain uses the same path, so an account's Ethereum and
  BSC addresses are one string and one deposit index.
- **Verified by a light client.** A deposit proof is a sync-committee
  finality update (finalized beacon header, committee bits and BLS
  signature) plus a Merkle-Patricia proof of the receipt holding the
  ERC-20 `Transfer` log, and the chain's view starts from an Ethereum
  checkpoint. Today the proof binds only the finalized checkpoint block,
  and native ETH has no receipt log, so only ERC-20 deposits in that block
  are credited; the rest is logged by the observer, not submitted.
- **Depth.** <span data-param="confirmations_eth">12</span> blocks
  (`confirmations_eth`), the knob shared with BSC.
- **Withdrawals.** EIP-1559 transactions, one per payout, signed by the
  vault key; the fee rate below is the gas price the observers estimate,
  in wei per gas.
- **Assets.** `ETH.ETH` (18 decimals) and `ETH.USDT` (6 decimals, the
  ERC-20), credited as `USDT`.
- **On the testnet** the Ethereum vault is registered only when the
  deploy has a Sepolia execution node, a beacon node and an Ethereum
  checkpoint; the box below says whether it is today. Explorer:
  [sepolia.etherscan.io](https://sepolia.etherscan.io).

<div class="live" data-live="chain:ETH"></div>

## BNB Smart Chain

- **Networks.** The BSC testnet (chain id 97) on the testnet; BNB Smart
  Chain (chain id 56) on mainnet. The chain id is bound into every signed
  transaction, which is how the signing policy tells BSC from Ethereum
  under the shared key.
- **Address.** The same `0x…` string as the account's Ethereum address,
  same deposit index: assigned on one, it is the other's too.
- **Observation.** Attestation only, no light client: the observers'
  quorum is the proof. BEP-20 transfers come from the receipt logs; BNB is
  read from the block's own transactions, so BNB sent by a contract (an
  internal transaction) is not seen.
- **Depth.** <span data-param="confirmations_eth">12</span> blocks
  (`confirmations_eth`): twelve 3-second blocks, past BSC's fast
  finality, about forty seconds.
- **Assets.** `BSC.BNB` (18 decimals) and `BSC.USDT` (18 decimals on
  BSC, the BEP-20), credited as `USDT`. The testnet's USDT contract is
  `0x337610d27c682E347C9cD60BD4b3b107C9d34dDd`.
- **Test coins.** The BNB Chain faucet for BNB, and test USDT from the
  contract above. Explorer:
  [testnet.bscscan.com](https://testnet.bscscan.com).

<div class="live" data-live="chain:BSC"></div>

## Tron

- **Networks.** Nile on the testnet; Tron on mainnet. **Not Shasta**:
  coins sent on Shasta to a Nile address string are on another ledger and
  are never seen. Addresses are base58check of `0x41` plus the 20-byte
  account hash, so they start with `T`.
- **Derivation.** BIP32 child at `m/195/<deposit index>` (SLIP-44 coin
  195), the same secp256k1 key family as Bitcoin and Ethereum.
- **Observation.** Attestation only: Tron has no practical light client,
  so the observers' quorum is the proof. TRX comes from `TransferContract`
  transactions, USDT from the TRC-20 `Transfer` log of the transaction
  info, read through TronGrid-style account routes.
- **Depth.** <span data-param="confirmations_tron">19</span> blocks
  (`confirmations_tron`), Tron's solidification depth, about one minute.
- **Withdrawals.** One transaction per payout as canonical protobuf,
  `txid = sha256(raw_data)`; the fee below is the per-transaction cost the
  observers budget, in sun (a millionth of a TRX), covering bandwidth and,
  for USDT, energy.
- **Assets.** `TRON.TRX` (6 decimals, sun) and `TRON.USDT` (6 decimals,
  the TRC-20), credited as `USDT`. The Nile USDT contract is
  `TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf`.
- **Test coins.** The Nile faucet at nileex.io for TRX and USDT.
  Explorer: [nile.tronscan.org](https://nile.tronscan.org/#).

<div class="live" data-live="chain:TRON"></div>

## Solana

- **Networks.** Devnet on the testnet; mainnet-beta on mainnet. An address
  is the base58 of a 32-byte Ed25519 public key.
- **Key.** The one vault that is not secp256k1: an Ed25519 key, held as
  FROST threshold shares rather than CGGMP21. Ed25519 has no non-hardened
  children, so Keel uses its own additive rule, which anyone can run from
  the registered `(public_key, chain_code)`: for each path element `i`
  under parent `P`, `t = SHA-512("keel-ed25519-child" || chain_code || P ||
  i) mod l` and `child = P + t·B`. The path shape is `m/501/<deposit
  index>`. The signature is a standard Ed25519 signature under the child
  key.
- **Observation.** Attestation only. The observers read **finalized**
  transactions, which already sit about 32 slots behind the tip; the
  chain's own depth check reuses `confirmations_tron`
  (<span data-param="confirmations_tron">19</span> slots).
- **Tokens.** SPL tokens land in the deposit address's **associated token
  account**, which the sending wallet creates; `SOL.USDC` is the devnet
  mint `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`.
- **Transaction ids.** The chain's `tx_hash` is the SHA-256 of the
  signature; the native id a Solana explorer knows is the base58
  signature, carried as `external_tx` on deposits and outbounds.
- **Withdrawals.** Legacy transaction messages for System and SPL Token
  transfers; the fee below is lamports per signature.
- **Assets.** `SOL.SOL` (9 decimals, lamports) and `SOL.USDC` (6 decimals).
  USDC is a Solana asset, not part of the unified `USDT`.
- **Test coins.** `solana airdrop` or faucet.solana.com for SOL, the Circle
  faucet for devnet USDC. Explorer:
  [solscan.io?cluster=devnet](https://solscan.io/?cluster=devnet).

<div class="live" data-live="chain:SOL"></div>

## Same on every network

- **One deposit index per account per network.** `RequestDepositAddress {
  chain }` records the next free index; asking again returns the same one.
  The address is read from `GET /v1/vaults/{chain}/addresses?owner=` or
  `/{index}`, never derived in a wallet ([deposits](../deposits/)).
- **Custodial clients have their own vaults** on the same networks, with
  the same derivation and the same observers, signed by the client's own
  signer (`RegisterCustodyVault`, `RequestCustodyAddress`). A rotation
  keeps every account's index and changes the string derived from it
  ([models](../models/)).
- **Large deposits.** Above
  <span data-param="large_deposit_usd_micro" data-unit="usd_micro">$50,000</span>
  (`large_deposit_usd_micro`) a credit is held for
  <span data-param="large_deposit_delay_blocks">600</span> blocks before it
  is spendable.
- **Outbounds are batched** per network every
  <span data-param="outbound_batch_interval_blocks">20</span> blocks; the
  observers build the transaction, the signers check it against the chain's
  own outbound rows before signing, and the quorum confirms it. The flat
  withdrawal fee is
  <span data-param="withdraw_flat_fee_usd_micro" data-unit="usd_micro">$1</span>
  (`withdraw_flat_fee_usd_micro`), in the withdrawn asset, on top of the
  network cost; a client's users have it waived while the client's plan
  runs.
- **Halts** are per asset: governance can stop deposits and payouts of one
  asset on one network while the rest of the network goes on; the `halted`
  list in each box is that.
- **Sent on the wrong network.** Anything that lands at a deposit address
  on any network, in any token, is the owner's; `RequestRecovery` moves it
  to an address of theirs without anyone approving
  ([deposits](../deposits/#sent-on-the-wrong-network)).
- **The per-network switch.** A client turns a network on for its users
  only when `GET /v1/ready/{chain}` says `ready`; the reasons list names
  what is missing (no vault, no checkpoint, a stale checkpoint, a halt).
  That is the call the boxes above show ([clients](../clients/)).
- **Adding a network** is a chain upgrade: an address derivation, an
  observation proof or attestation, a transaction builder and a signing
  policy ([how it works](../how-it-works/#10-adding-support-for-a-new-network)).

<script defer src="../../networks.js?v=1" data-rpc="https://testnet.keelchain.com/rpc" data-api="https://testnet.keelchain.com/api"></script>
