# Changelog

Testnet resets and binary upgrades are announced here before they happen.
Dates are UTC.

## Unreleased

- Chain: genesis lists a KUSD book for every non-dollar vault asset, not
  only BTC and KEEL: `ETH-KUSD`, `TRX-KUSD`, `BNB-KUSD`, `SOL-KUSD`,
  `USDC-KUSD`, with ticks suited to each price scale (`pair_with_tick`).
  USDT stays unlisted: it moves through the KUSD basket at par. Genesis
  changes; the testnet deploy needs `reset_chain`.
- Chain: **time-weighted reference price.** Settlement (surcharge
  thresholds, the withdrawal flat fee, budget earned per dollar, the
  liquidity pool's mark, the buyback) reads a time-weighted average of each
  pair's fills over `prices.twap_window_secs` (default 10 minutes) instead
  of the last print; the book still shows and trades at the last fill.
  `GET /v1/markets/{pair}` adds `reference_price`. Snapshot schema 11;
  schema 10 migrates with the window at 0 (last fill, as before).
- Explorer: the market page has a **Trade** card: connect Keel Wallet, place
  a limit or market buy or sell on the pair's book, see and cancel your
  open orders on it. The KEEL-KUSD page is where KEEL is bought and sold.
- Chain: **staking emission.** The genesis `validator_bootstrap` bucket
  (10% of the supply) pays validators, observers and delegators
  `emission.validator_bps_per_year` (default 10%) of what remains per year,
  accrued at each epoch boundary into `validator_rewards` and paid by the
  epoch distribution (`EmissionPaid`, ledger type `emission`). Nothing is
  minted. Snapshot schema 10; schema 9 migrates with the rate at 0.
- Chain: **client usage prices are non-zero by default** (0.01 KEEL per
  filled order, 0.1 KEEL per release, withdrawal and deposit address, 1 KEEL
  per ruling), so the protocol-fee waiver is paid for by clients in KEEL and
  the fee split sees revenue from client flows. Governance parameters;
  existing chains keep their values.
- Chain: **the free budget is an onboarding grant, not a lifetime
  allowance.** `budget.base` defaults to 500 actions (was 10,000). With a
  per-address grant and near-free keypairs, any volume could be run for
  free by spreading it over addresses; capacity at volume now comes from
  locked KEEL, which is linear in KEEL and additive across addresses.
  Existing chains keep their value; governance moves it.
- Chain: **an address must hold KEEL before its free budget counts.**
  `budget.activation_keel` (0.1 KEEL by default; spendable, locked,
  unlocking and bonded KEEL and the offer and proposal deposits count) is
  required for every budget-spending action, cancels, observer actions and
  a session key's self-revocation excepted; below it the chain answers
  `NOT_ACTIVATED`. Without it a budget per address was a budget per
  keypair. Clients send the dust with the attestation (`docs/clients.md`).
- Chain: **fee mode.** `budget.base` set to 0 by governance removes the
  free budget; an address with no earned or locked capacity then pays
  `budget.action_fee_keel` (0.001 KEEL by default) per action through the
  fee split, taken before the module runs (`ActionFeePaid`, ledger type
  `action_fee`). The free budget stays the default.
- Chain: **every limit that was a constant is a governance parameter**
  (`state.limits`, snapshot schema 9; schema 8 migrates with the floor off
  and every cap at its old value): `sessions.max_per_principal`,
  `sessions.max_secs`, `p2p.min_payment_window_secs`, `p2p.max_terms_chars`,
  `p2p.max_margin_bps`, `disputes.max_evidence`, `gov.max_title_chars`,
  `gov.max_description_chars`, `transfer.max_memo_bytes`, each with bounds.
  `GET /v1/params` lists them; `GET /v1/accounts/{addr}` reports
  `activated` and the floor. New `docs/governance.md` lists everything
  governance can change, with defaults, and what it cannot. Testnet: the
  genesis changes, so the next deploy needs `reset_chain`.
- Chain: **generations of a custody vault are separate pools; nothing is
  ever moved between them.** `RotateCustodyVault` with `sweep: true` and
  `ObserveCustodySweep` are refused (`INVALID`); the observers no longer
  build sweeps, and the signing policy lost the `*Sweep` contexts and the
  `sweep` flag of the consolidation contexts, so a custody move can only
  pay the signing key's own hot address or deposit index. A draining
  generation empties through the withdrawals the chain points at it (the
  oldest draining generation first, one generation per transaction). The
  `sweep`/`swept`/`pending_sweeps` fields left `GET /v1/custody`; the
  `sweep` field stays in the action encoding and must be `false`. CLI:
  `custody rotate` lost `--sweep`. Explorer: the rotate form lost its sweep
  box. Docs: `models.md` "Rotating to a fresh pool" states the rule, the
  reasons, and what a rotation changes from the exchange's side (Keel
  accounts never; external deposit strings yes; old and new addresses
  coexist while the old generation drains; withdrawals paid from whichever
  generation holds funds; when a saved old address stops working).
- Wallet: a custodial client's users get the custody flow in Keel Wallet
  (docs/wallet.md §5c): Receive hands out the address in the client's
  vault, Send withdraws custody balances through the client's vault and
  offers *Custody ↔ market* (the float swap). `/v1/accounts/{addr}` now
  carries `attester` and `custodian`. SDK: `requestCustodyAddress`,
  `withdrawCustody`.
- Chain: self-service recovery of coins sent to a deposit address on a
  network the chain does not serve or in a token it does not list
  (`RequestRecovery` by the index's owner, or by the custodian for an
  account in its custody; `ObserveRecovery` by the observers; `GET
  /v1/recovery`). The chain checks the index is the requester's and opens
  a row; the observers move the balance out of the index's own key to the
  requester's destination under new policy contexts bound to the row,
  and the quorum closes it; rows nobody serves expire. Unserved EVM
  networks come from the observers' `[recovery] evm` list, with the
  address paying its own gas. Parameters `recovery.fee_keel` and
  `recovery.expiry_blocks`. Snapshot schema 8 (schema 7 migrates). Wallet:
  *Deposits* → *Sent on the wrong network?*.
- Chain: Solana (`Chain::Solana`, assets `SOL.SOL` with 9 decimals and
  `SOL.USDC` with 6). The vault key is Ed25519, 32 bytes, and the signer
  gains a second threshold protocol beside CGGMP21: FROST
  (`keel-tss keygen --scheme eddsa`, a second share file, one `serve`
  for both). Deposit addresses come from an additive child derivation
  (`keel_chains::sol`) that a FROST signer applies to its shares. The
  observer watches finalized transactions' balance deltas, pays out from
  the hot address with `TransferChecked` and idempotent associated-account
  creation, and consolidates deposit indexes with two-signer messages;
  the signing policy parses the message and binds it to the batch's rows
  or to the hot addresses. Attestation only; depth shares
  `confirmations_tron`. SDK, wallet, explorer (solscan), indexer, the
  ceremony (both keys) and the deploy (`SOLANA_RPC_URL`,
  `SOLANA_USDC_MINT`) follow. The testnet bridges Solana devnet.
- Chain: deposit and outbound observations carry `external_tx`, the
  transaction id as the external chain writes it when the 32-byte
  `tx_hash` is only its hash (Solana: the base58 signature; `null`
  elsewhere). It is part of the attestation the observers vote on, kept
  on the outbound, emitted in `DepositObserved` and `OutboundConfirmed`,
  served by `/v1/vaults/deposits` and `/v1/vaults/outbounds`, stored by
  the indexer, and used by the explorer and wallet for Solana links.
  Snapshot schema 7 (schema 6 migrates); the action encoding changed, so
  the next deploy resets the testnet.
- Chain: BNB Smart Chain (`Chain::Bsc`, assets `BSC.BNB` and `BSC.USDT`,
  18 decimals each). BSC shares Ethereum's key path and deposit indexes:
  an account's BSC deposit address is its Ethereum address, asking on
  either chain assigns both, and a BSC vault must be registered with the
  Ethereum vault's key. Attestation only (no light client); the depth is
  `confirmations_eth`. Observer: `[bsc]` config (the `[ethereum]` shape),
  native BNB deposits read from full blocks, payouts and custody moves
  through the EVM path keyed by chain id (56/97); the signing policy reads
  the chain from the transaction's chain id. SDK, wallet (Receive button,
  balances), explorer (bscscan links), indexer and the deploy
  (`BSC_RPC_URL`, `BSC_USDT_CONTRACT`, the BSC vault registered with the
  others) follow. The testnet bridges the BSC testnet (chain id 97).
- 0.2.0 (activation by `SoftwareUpgrade { "0.2.0", H }`; fresh networks
  seed it at genesis): the pooled house-maker inventory (`DepositLiquidity`,
  `WithdrawLiquidity`, `liquidity.*` parameters, `GET /v1/liquidity`,
  `/v1/liquidity/{addr}`); clients keep all their fees, the chain waives
  its protocol fee for attested accounts and bills the client per flow
  (`clients.usage_fill_keel`, `usage_release_keel`, `usage_ruling_keel`,
  `clients.protocol_fee_waived`), retail caps lifted; `clients.*` and
  `liquidity.*` keys votable by `ParamChange`; `GET /v1/params` carries
  them; snapshot schema 6 (`State.liquidity`, new `clients` fields). The
  next deploy resets the testnet.
- Docs: the whitepaper, how-it-works 15 and models.md describe the two
  places a user's bridged coins can sit, in Keel or in their wallet.
- Docs: the whitepaper (draft v0.2) covers client custody, the float swap,
  fresh-pool rotation and per-offer arbitration; how-it-works 9, 11 and 15
  point at the swap; models.md walks a trade between the two models.
- Wallet: coins can be held *in your wallet* as well as *in Keel*. The
  wallet derives Bitcoin (BIP84) and Tron (BIP44) keys from the same seed
  phrase, shows their balances from the public chain APIs, and moves coins
  into Keel by building, signing and broadcasting the deposit itself; the
  Send card's withdrawal gets a one-click "Move to your wallet" destination.
  Settings → "New deposits go to" chooses which address Receive shows; the
  default stays Keel. Nothing on the chain changes. The popup is now five
  tabs (Home, Send, Deposits, Wallet, Settings); Send lists the bridged
  assets whether or not they are held and gains a "Move to your wallet" kind
  with the destination locked to the own address.
- Deposits in flight: the wallet's home screen and the explorer's account
  page (*Deposits* tab) show each incoming deposit from the moment the
  external network sees it (looked up on mempool.space or TronGrid from the
  browser) through its confirmations, the observers' votes and the credit,
  with the network named on every deposit address (Bitcoin signet and Tron
  Nile on the testnet). SDK: `externalNetworks`, `fetchIncoming`,
  `depositProgress`. The explorer's Bitcoin links for the testnet now point
  at signet, not testnet4. The indexer lists each deposit address once.
- Wallet: a deposit being processed stays in the Deposits tab at its last
  known stage when one refresh cannot see it (a refused public-API call, or
  the gap between the last confirmation and the observers' report), instead
  of vanishing for a refresh and coming back credited.
- Wallet: Home hides empty non-spendable rows (an emptied withdrawal escrow
  used to linger as "Withdrawal in flight 0").
- Wallet: the own address's USDT balance is read from the token contract,
  so an address that holds only tokens (no account record on Tron yet)
  shows what it holds instead of zero.
- Wallet: a Tron move to Keel is signed with the 27-offset recovery byte
  java-tron requires; the first build used the raw recovery id.
- Wallet: balances name what each ledger account means ("Available",
  "Withdrawal in flight", "In trade escrow", "In open orders") instead
  of the ledger's own account names such as `sendout_escrow`.
- Wallet: the unlocked session survives the browser evicting the service
  worker (mirrored in `chrome.storage.session`, memory only, cleared on
  browser exit), so the password is asked when the auto-lock timer says,
  not every half minute of idleness.
- Wallet: a refreshed look in the site's palette with a dark theme, a brand
  header, pill tabs, cards with depth, clearer inputs and buttons; the
  Send tab's result shows a copyable, linked transaction id.
- Wallet: the Lock button handed the popup a bare acknowledgement instead
  of the wallet state, so the popup rendered the onboarding screen until
  its next poll; lock now returns the locked state.
- Wallet: the approval popup describes the custody, client-fee and
  arbitration actions in words (swap, custody withdrawal, vault
  registration and rotation, named arbitrators, retail fees) instead of a
  generic field list, with a warning on the ones that move funds or
  change what backs them.
- Dev: `sync-e2e.sh` compared the follower's tip hash from one status call
  with the validator's block at a height read a moment earlier, so a block
  arriving in between failed the run; it now compares the same block on
  both nodes.
- Dev: `infra/dev/devnet.sh stop`, `kill` and `restart` wait for the nodes
  to exit and release their p2p, metrics and RPC ports, and `start` refuses
  busy ports instead of launching validators that cannot bind (the CI e2e
  job runs the devnets back to back and the failover check raced the
  state-sync nodes' shutdown).
- Node: versioned snapshots with a migration path (schema 3), follower nodes
  (`--role follower`, `--extra-peers`), state sync from a peer's snapshot
  (`--sync-from`, `--sync-verify`, `/v1/sync/*`), journal segments with
  `--retain-blocks`, `oldest_block` and `version` in `/v1/status`, governed
  upgrades (a node stops at an activation height it was not voted in for).
- API: filtered, sequenced WebSocket subscriptions with gaps and replay on
  the node and the indexer; `/v1/ready/{chain}`; `/v1/clients/{addr}`,
  `/v1/treasury` and `/v1/custody`; receipts beyond the node's window point
  at the indexer; action submission can require an API key on the public
  testnet.
- Chain: clients as fee recipients (`SetClientFee`, retail legs on releases,
  fills and withdrawals), usage prices for custody rails, the epoch buyback
  that turns fee income into KEEL, the `KEEL-KUSD` pair at genesis.
- Custody: fresh-pool rotation (schema 5, `State.pools`): `RotateCustodyVault`
  opens a new generation of a client's vault with a key of its own, the old
  one drains (watched, credited, paying payouts one key per transaction)
  until its grace is over and the observers report it empty; `sweep` moves
  the old coins to the new key (`ObserveCustodySweep`, fee charged to the
  client) or leaves the pools unlinked; `CancelCustodyRotation`,
  `RetireCustodyGeneration`; `pools.grace_blocks`, `pools.usage_rotation_keel`;
  `/v1/custody` lists live generations, `?gen=` and `addresses/lookup`;
  `gen` on batches. Registering over an existing vault is refused.
- Observer: a remembered Tron payout the network never saw is rebroadcast
  within its expiry window and dropped after five minutes so the outbound
  is built and signed again (a record written before a failed broadcast
  had pinned the testnet's first Tron withdrawal in `Batched`); an
  Ethereum payout whose broadcast was lost is resent like Bitcoin's.
- Observer: Tron transactions are broadcast through `/wallet/broadcasthex`
  with the signed protobuf. The JSON endpoint rebuilds a transaction from
  its `raw_data` object and ignores `raw_data_hex`, so the hex-only body
  the observer sent came back as a NullPointerException: no Tron payout or
  consolidation had ever left an observer. Found on the testnet's first
  Tron withdrawal.
- Observer: the network vault's Ethereum and Tron deposit indexes are
  consolidated into the hot address the payouts spend from (tokens first,
  with gas funding when an index lacks it, then the native coin), the same
  pass client vaults already had. Before this the first Tron withdrawal on
  the testnet sat in `Batched` for good: every coin was on the users'
  indexes and index 0 held nothing to pay from or to pay energy with.
- Observer: one address book per live custody generation, batches paid
  from the generation the chain prefers with fallback to the active key,
  sweeps on Bitcoin, Ethereum and Tron, Ethereum/Tron consolidation of
  deposit indexes into the hot address with gas funding, empty-generation
  reports; a sweep one observer broadcast is verified by every other
  observer on its own node (the chain lists the observations collecting
  votes) so the quorum is reached on every chain; `intervals.moves_secs`.
  Signer policy: `BtcSweep`, `EthSweep`,
  `EthConsolidate`, `EthGasFund`, `TronSweep`, `TronConsolidate`,
  `TronGasFund` contexts; a sweep may only pay the active generation's own
  address (`keel-tss` fetches it from the node).
- Chain: per-offer arbitration (schema 4, `State.arbitration`): an offer
  names bonded arbitrators (`SetOfferArbitrators`), the taker accepts them by
  taking it or picks one (`ChooseArbitrator`), the bound arbitrator rules;
  offers naming nobody keep the elected set; `BanArbitrator` proposal;
  `arbitration.max_named` param; `arbitrators[]` on offers, `arbitrator` and
  `arbitrator_candidates[]` on trades, `arbitrators_bonded`/`arbitrators_banned`
  on `/v1/gov/roles`.
- Custody: the float swap, `SwapCustody`, moves a balance between a
  client's custody and the network vault against the client's own floats in
  one posting, with no external transaction and both reserve checks
  untouched, so custodial users trade on the chain's book and escrow;
  `floats[]` on `/v1/custody/{addr}`, the `CustodySwapped` event, the SDK's
  `swapCustody`/`custody`, `keel send custody swap`.
- Custody, client-owned vaults: `RegisterCustodyVault`,
  `RequestCustodyAddress`, `ObserveCustodyDeposit`, `WithdrawCustody`; the
  `custody` and `custody_escrow` account types; per-vault reserve checks and
  halts; observers watch client vaults and sign their batches through the
  client's own signer.
- Custody: the threshold signer enforces a signing policy against the
  chain's outbound rows; observers attach the transaction context to every
  request; multi-host deploy with a WireGuard mesh, a TSS ceremony workflow
  and governance checkpoints voted by every host.
- Operations: daily encrypted backups with a restore script, health metrics
  for node_exporter with an optional Prometheus, Alertmanager and Grafana
  stack, a vote timer for pre-approved proposals, a staging environment
  input on the deploy, client re-onboarding after a reset.
- Wallet 1.2.0: the old client alias and client domains are gone; any
  site can be enabled from the wallet; Send and Withdraw from the popup;
  custom networks; unlisted Firefox signing and self-hosted updates.
- Wallet 1.2.1: the popup offers "Enable on this site" on its home screen
  when opened on a site that cannot see the wallet yet, and enabling puts
  the provider on the open page without a reload.
- SDK 0.1.x: `KeelSocket`, `IndexerClient`, `ready()`, the connect kit
  (`connectWallet`, `loginChallenge`, `verifyLoginChallenge`,
  `requestSession`, `mountConnectButton`), the custody actions.
- Docs: `models.md` (the two client models), `clients.md` (the integration
  contract), `connect-keel-wallet.md`, the WebSocket reference, all rendered
  under keelchain.com/docs.
- Testnet: the next deploy after this changelog entry resets the chain
  (schema 3 and the new genesis roles); onboarded clients are listed in
  `KEEL_CLIENTS` and re-onboarded by the deploy.

## 2026-09-25

- First public deploy of the Keel testnet from GitHub Actions; SDK
  `@keelchain/sdk` 0.1.0 published; wallet 1.1.1 submitted.
