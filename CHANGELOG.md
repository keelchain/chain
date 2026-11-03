# Changelog

Testnet resets and binary upgrades are announced here before they happen.
Dates are UTC.

## Unreleased

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
