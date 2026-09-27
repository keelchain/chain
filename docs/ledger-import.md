# Importing a client ledger onto the chain

A client that moves an existing ledger onto the chain keeps its migration
playbook in its own repository, next to the code it describes. The chain-side
pieces such a playbook relies on are `keel genesis-from-export`
(`chain/crates/keel-cli/src/genesis_export.rs`), the vault module
(`chain/crates/keel-vm/src/modules/vaults.rs`) and the stable coin module
(`chain/crates/keel-vm/src/modules/stable.rs`).
