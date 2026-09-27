# Changelog

Testnet resets and binary upgrades are announced here before they happen.
Dates are UTC.

## Unreleased

- Node: versioned snapshots with a migration path (schema 2), follower nodes
  (`--role follower`, `--extra-peers`), state sync from a peer's snapshot
  (`--sync-from`, `--sync-verify`, `/v1/sync/*`), journal segments with
  `--retain-blocks`, `oldest_block` and `version` in `/v1/status`, governed
  upgrades (a node stops at an activation height it was not voted in for).
- API: filtered, sequenced WebSocket subscriptions with gaps and replay on
  the node and the indexer; `/v1/ready/{chain}`; `/v1/clients/{addr}` and
  `/v1/treasury`; receipts beyond the node's window point at the indexer;
  action submission can require an API key on the public testnet.
- Chain: clients as fee recipients (`SetClientFee`, retail legs on releases,
  fills and withdrawals), usage prices for custody rails, the epoch buyback
  that turns fee income into KEEL, the `KEEL-KUSD` pair at genesis.
- Custody: the threshold signer enforces a signing policy against the
  chain's outbound rows; observers attach the transaction context to every
  request; multi-host deploy with a WireGuard mesh, a TSS ceremony workflow
  and governance checkpoints voted by every host.
- Wallet 1.2.0: the `window.stt` alias and client domains are gone; any
  site can be enabled from the wallet; Send and Withdraw from the popup;
  custom networks; unlisted Firefox signing and self-hosted updates.
- SDK 0.1.x: `KeelSocket`, `IndexerClient`, `ready()`, the connect kit
  (`connectWallet`, `loginChallenge`, `verifyLoginChallenge`,
  `requestSession`, `mountConnectButton`).
- Docs: `models.md` (the two client models), `clients.md` (the integration
  contract), `connect-keel-wallet.md`, the WebSocket reference, all rendered
  under keelchain.com/docs.
- Testnet: the next deploy after this changelog entry resets the chain
  (schema 2 and the new genesis roles); onboarded clients are listed in
  `KEEL_CLIENTS` and re-onboarded by the deploy.

## 2026-09-25

- First public deploy of the Keel testnet from GitHub Actions; SDK
  `@keelchain/sdk` 0.1.0 published; wallet 1.1.1 submitted.
