# Importing a client ledger onto the chain (SafeTheTrade as the worked example)

The migration and integration playbook is maintained in the marketplace
repository as `docs/ledger-import.md`, next to the code it describes
(`apps/marketplace/src/modules/chain`, migrations 0144–0146, the backoffice
Chain page). The chain-side pieces it relies on are `keel genesis-from-export`
(`chain/crates/keel-cli/src/genesis_export.rs`), the vault module
(`chain/crates/keel-vm/src/modules/vaults.rs`) and the stable coin module
(`chain/crates/keel-vm/src/modules/stable.rs`).
