# keel-observer

The observer-signer daemon of one Keel observer (docs/plan.md §4). It
follows the external chains, turns deposits to vault addresses into
`ObserveDeposit` actions with the proof the VM verifies, drives the
outbound pipeline (batch → sign through the TSS → broadcast → confirm →
`ObserveOutbound`) and reports network fees. `API.md` lists the KEEL RPC
routes it uses, the JSON it expects, and the upstream changes still
needed.

```text
keel-observer example-config > observer.toml
KEEL_OBSERVER_SECRET=<hex32> keel-observer run --config observer.toml
KEEL_OBSERVER_SECRET=<hex32> keel-observer register-vault --config observer.toml --chain BTC \
        --public-key 02… --chain-code … --signers a,b,c --threshold 2      # after the keygen ceremony
keel-observer addresses --config observer.toml --chain BTC                # index, address, owner
```

`KEEL_OBSERVER_SECRET` is the observer's ed25519 account key (the one
bonded as an observer on the chain; on a devnet `keel keygen --seed N`).

## Configuration

```toml
keel_rpc_url = "http://127.0.0.1:8545"      # any Keel node
tss_url     = "http://127.0.0.1:7100"      # keel-tss serve, or "local:<hex seed>" (dev signer)
state_file  = "/var/lib/keel-observer/state.json"

[intervals]            # seconds; defaults 30 / 20 / 15 / 300
sync_secs = 30         # vault registrations → address books, wallet imports
deposits_secs = 20
outbound_secs = 15
fees_secs = 300

[bitcoin]              # Bitcoin Core JSON-RPC (pruned is fine)
rpc_url = "http://127.0.0.1:18443"
rpc_user = "keel"
rpc_password = "keel"
network = "regtest"    # mainnet | testnet | signet | regtest
wallet = "keel-vault-watch"   # watch-only descriptor wallet the daemon creates
max_proof_headers = 144
fee_target_blocks = 2
fallback_sat_per_vb = 2

[ethereum]
rpc_url = "http://127.0.0.1:8545"
beacon_url = "http://127.0.0.1:5052"   # Lighthouse etc.; omit on anvil (no proofs possible)
network = "regtest"
log_window = 2000
hot_index = 0
tokens = [{ symbol = "USDT", contract = "0x…" }]

[bsc]                                   # BNB Smart Chain: the [ethereum] shape, attestation only
rpc_url = "https://data-seed-prebsc-1-s1.bnbchain.org:8545"
network = "testnet"                     # chain id 97; mainnet = 56
log_window = 2000
native_scan_blocks = 64                 # newest full blocks read per pass for native BNB deposits
hot_index = 0
tokens = [{ symbol = "USDT", contract = "0x337610d27c682E347C9cD60BD4b3b107C9d34dDd" }]

[solana]                                # attestation only; Ed25519 vault key (FROST share)
rpc_url = "https://api.devnet.solana.com"
network = "testnet"                     # devnet; mainnet = mainnet-beta
priority_micro_lamports = 0             # SetComputeUnitPrice when > 0
signature_limit = 50                    # newest signatures read per address and pass
hot_index = 0
tokens = [{ symbol = "USDC", contract = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU", decimals = 6 }]

[tron]
api_url = "https://nile.trongrid.io"   # TronGrid-style /v1 routes for deposits
api_key = "…"
fee_sun = 15000000
fee_limit_sun = 100000000
hot_index = 0
tokens = [{ symbol = "USDT", contract = "T…" }]

[outbound]
enabled = true              # false on a pure observer that never signs
leader_timeout_secs = 900   # a follower takes over a batch after this

[vault_fallback]            # devnet only, see API.md "Required upstream changes"
signers = ["<hex>", "<hex>", "<hex>"]
threshold = 2
# public_key / chain_code, or omitted with tss_url = "local:…"
```

Chains not present in the file are not watched. Secrets never live in
the file. The state file (`state.rs`) holds idempotency keys of submitted
actions, broadcast transactions (with the raw bytes for rebroadcast) and
scan cursors; it is rewritten atomically after every change and must
survive restarts, otherwise the daemon may resubmit observations (the VM
rejects duplicates, so this costs only budget) or forget an in-flight
transaction (it is found again by the follower path).

## Loops

`daemon.rs` runs four tokio loops; each pass tolerates node errors and
retries on the next tick.

| Loop | What one pass does |
|---|---|
| sync | `GET /v1/vaults/{chain}` per configured chain → `AddressBook` (every deposit index below `next_deposit_index`, plus index 0 = hot/change). For Bitcoin, imports the new addresses as `addr()` descriptors into the watch-only wallet (`importdescriptors`, rescan from genesis on regtest). Refreshes `GET /v1/params`. |
| deposits | Bitcoin: `listunspent` on the wallet, one `ObserveDeposit` per UTXO on a user index that is deep enough. Ethereum: `eth_getLogs` for ERC-20 `Transfer` to book addresses in a window, proof from the beacon finality update (see below). Tron: TronGrid `/v1/accounts/{addr}/transactions[/trc20]`, attestation only. |
| outbound | `GET /v1/vaults/outbounds?status=Batched`. Per batch the leader (`signers[batch_id % n]`) builds and signs the transaction, remembers it, broadcasts; followers look for the payment on chain and take over after `leader_timeout_secs` × their distance from the leader. Every signer that knows the transaction submits `ObserveOutbound` once it is `confirmations + 1` deep. Bitcoin: one transaction per batch, one in flight at a time, inputs are any watched UTXOs (largest first), change to index 0, fee from the wallet's `gettransaction` split evenly. Ethereum/Tron: one transaction per outbound from the hot address (index 0). |
| fees | `ReportNetworkFee` per chain: `estimatesmartfee` (sat/vB, fallback on regtest), `eth_feeHistory` (base + median tip, wei/gas), a constant for Tron (sun). The chain uses the median across observers. |

Submissions go through `rpc::Submitter`: nonce cached locally, resynced
from `/v1/accounts/{addr}` after any refusal; idempotency key per
observation (`deposit:BTC:<txid>:<vout>`, `outbound:<id>`) so nothing is
sent twice.

## Proofs per chain

**Bitcoin (real, verified in-VM).** `gettxoutproof` gives a
`MerkleBlock`; the daemon splits out the partial merkle tree and the tx
position, fetches the raw 80-byte headers from the containing block to
the tip (capped at `max_proof_headers`), and submits
`Proof::Bitcoin { headers, merkle_proof, tx_index }`. `keel-lc-btc`
checks PoW and linkage, the merkle root, and `depth ≥
confirmations_btc`. Regtest headers pass because each header is checked
against its own target; a mainnet deployment relies on the VM's
checkpointed `HeaderChain` (not set by the devnet genesis, see API.md).
Known limitation: the VM keys deposits by `(chain, tx_hash, tx_index)`
where `tx_index` is the transaction's position in the block, so a
transaction paying two vault outputs is credited once.

**Ethereum (real proof format, needs a beacon node).** The VM verifies
`keel_lc_eth::EthDepositProof`: a light-client finality update (attested
and finalized beacon headers, finality branch, sync-committee bits and
BLS signature, the committee's public keys), the execution-payload field
roots and branch of the finalized header, and an MPT proof of the
receipt holding the ERC-20 `Transfer` log. The daemon reads
`/eth/v1/beacon/light_client/finality_update`, the committee from
`/eth/v1/beacon/light_client/updates` or `bootstrap`, and rebuilds the
receipts trie from `eth_getBlockReceipts`. Because the proof binds the
receipts root to the *finalized checkpoint block* only, a deposit in any
other block of the epoch has no proof: it is logged and recorded as
`unprovable:` in the state file, never submitted. Native ETH transfers
have no receipt log and are only logged. **anvil has no beacon chain**,
so with `beacon_url` omitted nothing is ever submitted for Ethereum;
outbound signing and broadcast still work against anvil. The VM side
also needs an ETH checkpoint (`set_eth_checkpoint`) and the token
contracts (`set_token_contract`), which no genesis or governance path
sets today (API.md).

**BNB Smart Chain (attestation only).** The `[ethereum]` code path with
`Proof::None`, which the VM accepts because BSC has no light client.
ERC-20 (BEP-20) transfers come from `eth_getLogs` as on Ethereum; native
BNB deposits are read from the newest `native_scan_blocks` full blocks of
each pass (`eth_getBlockByNumber` with transactions, `to` in the book,
`value > 0`, receipt status 1), so a transfer made by a contract (an
internal transaction) is not credited. Payouts, consolidation and sweeps
are the Ethereum ones with the chain id the node reports (97 testnet, 56
mainnet); the signing policy reads the chain from that id. BSC shares
Ethereum's key path (`m/60/<index>`), so the vault must be registered
with the Ethereum key and an account's address is the same on both.

**Solana (attestation only).** The vault key is Ed25519 (32 bytes), the
`--scheme eddsa` FROST share of `keel-tss`; the development signer
derives one from its seed. Deposit addresses are `keel_chains::sol`
children of the registered key. Each pass reads the newest
`signature_limit` finalized signatures of every watched address and of
its associated token accounts, fetches each transaction once, and attests
every positive balance delta (`preBalances`/`postBalances` for SOL,
`preTokenBalances`/`postTokenBalances` for tokens) as a deposit keyed by
`(SHA-256(signature), account index)`. Payouts are legacy messages paid by
the hot address (a `TransferChecked` plus an idempotent associated-account
creation for tokens), signed over the message bytes
(`POST /sign {"scheme":"eddsa","message":…}`), broadcast with
`sendTransaction` and settled from `getSignatureStatuses` at `finalized`;
a broadcast still unknown after 90 seconds (the blockhash lifetime) is
rebuilt. Consolidation of a deposit index is one message with two signers,
the hot address paying and the index authorizing, so no gas funding is
needed.

**Tron (attestation only, mocked proof).** `Proof::None`; the VM accepts
it because Tron has no light client (TIP-248 is still a draft). Depth is
`getnowblock` height minus the transaction's block, TRC-20 amounts are
matched against the `Transfer` log of `gettransactioninfobyid`. The
deposit scan uses the TronGrid-style `/v1/accounts/{addr}/transactions`
and `/transactions/trc20` routes, which a bare java-tron node does not
serve.

## Signing

`tss_url = "http://…"` posts `{digest, path}` to `keel-tss serve`
(`crates/keel-tss/README.md`). `tss_url = "local:<hex seed>"` is the
development signer: one BIP32 master key derived from the seed, children
at the same `m/<slip44>/<index>` paths, so the addresses are identical to
what a threshold key with the same `(public_key, chain_code)` would give.
`register-vault --local-seed <seed>` registers exactly that key. It is a
whole private key in one process and exists only for regtest/devnet runs
and tests.

## Running against local chains

### Regtest bitcoind (fully working, exercised by `infra/dev/custody-e2e.sh`)

```sh
bitcoind -regtest -datadir=/tmp/btc -rpcport=18443 -rpcuser=keel -rpcpassword=keel -fallbackfee=0.0001 -txindex=1
bitcoin-cli -regtest -rpcport=18443 -rpcuser=keel -rpcpassword=keel createwallet miner
bitcoin-cli … -rpcwallet=miner generatetoaddress 101 "$(bitcoin-cli … -rpcwallet=miner getnewaddress)"
```

Then, with the 4-node devnet up (`infra/dev/devnet.sh start`; its
validators are the genesis observers, keys `keel keygen --seed 0..3`,
threshold 3 of 4):

1. `register-vault --chain BTC --local-seed <seed> --signers <a0,a1,a2> --threshold 2`
   signed by observer 0.
2. Run three daemons (seeds 0, 1, 2) with the **same** `local:<seed>` and
   different `wallet` names, each pointed at any node. Three daemons are
   needed because the devnet quorum is 3.
3. A user submits `RequestDepositAddress { chain: Bitcoin }`;
   `keel-observer addresses` prints the address of its index; send
   regtest coins and mine `confirmations_btc + 1` blocks; the user's
   `BTC.BTC` deposit balance appears once the third proof lands.
4. `keel withdraw BTC.BTC <bcrt1…> <sats>`; after the next batch
   boundary the leader signs and broadcasts; mine 3 blocks; the escrow
   empties once three `ObserveOutbound`s land.

`infra/dev/custody-e2e.sh` automates all of it, including downloading
Bitcoin Core into `~/.local/bin` when `bitcoind` is missing. Devnet
shortcut: all three daemons share one seed, so any of them can sign
alone — a real deployment runs one `keel-tss serve` per observer and the
leader's daemon coordinates the threshold signature.

### anvil (Ethereum)

```sh
anvil --port 8545 --chain-id 31337
```

Configure `[ethereum]` with `rpc_url`, no `beacon_url`, `network =
"regtest"`, and the token contracts you deployed. What works: address
derivation, native-balance watching (logged only), fee reports, and the
outbound path (fund the hot address, index 0, with ETH and tokens;
`Eip1559Tx` uses `eth_chainId`). What does not: any deposit credit —
there is no beacon node, hence no finality update, hence no proof, and
the VM has no ETH checkpoint or token contracts anyway. Testing the
proof path needs Sepolia with a Lighthouse beacon node, an ETH
checkpoint in genesis, and a deposit that lands in a finalized
checkpoint block.

### Tron devnet

A private java-tron FullNode (`-p 8090`) serves `/wallet/getnowblock`,
`/wallet/gettransactioninfobyid` and `/wallet/broadcasttransaction`,
which is enough for outbound and confirmation tracking, but not the
`/v1/accounts/...` deposit routes. For deposits point `api_url` at Nile
(`https://nile.trongrid.io` with an API key) and fund a derived address
from the Nile faucet. `confirmations_tron` (19) means ~1 minute.

## Tests

`cargo test -p keel-observer` covers parsing of every node response used,
the Bitcoin proof end to end against `keel-lc-btc` on an in-process
mined regtest chain, the receipts-trie proof against `keel-lc-eth`, the
beacon finality-update parsing and proof assembly, deposit passes with
mocked nodes (idempotency, depth), batch signing with the dev signer
(signatures verified under the derived keys), the leader rotation, the
receipts fallback, config parsing, state persistence, and the submitter's
nonce handling. No test touches a network.
