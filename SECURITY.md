# Security

Keelchain is a public testnet. Nothing on it has monetary value, and the
consensus, vault and signing code has not been audited. Treat every finding as
worth reporting anyway: the testnet exists to find them before mainnet.

## Reporting

Email **mohab@keelchain.com** with a description, the affected component
(`chain/crates/<name>`, `apps/wallet-extension`, `sdk/ts`, `infra/`), steps to
reproduce, and the impact you see. Encrypt with your own key if you like and
say so; I will reply from the same address. Please do not open a public issue
for anything that could be exploited on the testnet or in the wallet.

You will get an acknowledgement within three days and a fix, a mitigation or
an explanation within thirty. There is no bounty programme yet; credit in the
release notes is given if you want it.

## Scope

- Consensus, state machine, ledger, order book, escrow and dispute logic.
- Vaults: observer attestations, light-client verification, threshold signing,
  deposit and payout builders.
- Keel Wallet: key handling, the approval flow, the `window.keel` provider.
- The indexer and node RPC, the SDK, the deploy scripts under `infra/`.

Out of scope: the testnet box itself (denial of service, resource exhaustion)
and third-party clients that build on the chain, which have their own contacts.

## Keys and secrets

No key material lives in this repository. The testnet's keys come from a
GitHub environment and are written only on the box by the deploy. If you find
a secret in the repository, in a build artifact or in a published package,
report it the same way; it is a leak by definition.
