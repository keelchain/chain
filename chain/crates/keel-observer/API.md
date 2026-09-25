# keel-observer ↔ Keel node API

Everything the daemon needs from an Keel node, over the HTTP API of
`crates/keel-rpc/API.md`. Hex strings are lowercase without `0x` unless
stated; amounts are integers in smallest units (numbers or strings).

## Routes used

| Route | Used by | Fields read |
|---|---|---|
| `GET /v1/status` | connect, receipts fallback | `chain_id` (u32, signed into every action), `height` |
| `GET /v1/params` | sync | `confirmations_btc`, `confirmations_eth`, `confirmations_tron`, `outbound_batch_interval_blocks` (defaults 2 / 12 / 19 / 20 when missing) |
| `GET /v1/accounts/{addr}` | submitter | `nonce` |
| `POST /v1/actions` | submitter | body `SignedAction` JSON; response `admitted` (bool; defaults to `status < 300`), `tx_id`, `error` |
| `GET /v1/vaults/{chain}` | sync, `addresses` | see below |
| `GET /v1/vaults/outbounds?status=Batched` | outbound | see below |
| `GET /v1/blocks/{height}/receipts` | receipts fallback only | `receipts[].ok`, `receipts[].events[]` |

`{chain}` is `Chain::as_str()`: `BTC`, `ETH`, `TRON`. Chain names in
JSON are accepted both as that and as the serde variant (`Bitcoin`, …).

### `GET /v1/vaults/{chain}` — expected shape

```json
{
  "chain": "BTC",
  "vault": {
    "chain": "Bitcoin", "epoch": 1,
    "public_key": "02…33 bytes…", "chain_code": "…32 bytes…",
    "signers": ["<observer hex>", "…"], "threshold": 2, "registered_height": 17
  },
  "next_deposit_index": 3,
  "deposit_owners": { "1": "<owner hex>", "2": "<owner hex>" }
}
```

`vault: null` means no vault is registered. `public_key`/`chain_code`
may also be byte arrays (serde form). `chain_code` is mandatory: the
daemon derives addresses only for HD vaults. `deposit_owners` lists
every assigned index (index 0 is never assigned and is the hot/change
address).

### `GET /v1/vaults/outbounds?status=` — expected shape

```json
{ "outbounds": [ {
  "id": 4, "owner": "<hex>", "asset": "BTC.BTC", "chain": "Bitcoin",
  "to": "bcrt1q…", "amount": 20000000,
  "fee_asset": "BTC.BTC", "fee_estimate": 400,
  "status": "Batched", "batch_id": 2, "created_height": 310, "tx_hash": null
} ] }
```

`batch_id` groups the rows of one Bitcoin transaction and picks the
leader (`signers[batch_id % n]`); `fee_estimate` is the amount locked in
escrow for the network fee, which followers report as `fee_paid` when
they cannot read the real fee from the chain.

## Actions submitted

All as `Action` variants in `SignedAction { envelope: { signer, nonce,
chain_id, action }, signature }` (`crates/keel-actions`):

- `ObserveDeposit(DepositObservation { chain, asset, tx_hash, index, deposit_index, amount, external_height, tip_height, proof })`
  — `proof` is `Bitcoin { headers, merkle_proof, tx_index }`,
  `Ethereum { proof: borsh(EthDepositProof) }` or `None` (Tron).
  Bitcoin `index` equals `tx_index` (the VM requires it).
- `ObserveOutbound(OutboundObservation { outbound_id, tx_hash, external_height, tip_height, fee_paid, success })`
- `ReportNetworkFee { chain, fee_rate }` — sat/vB, wei/gas, sun.
- `RegisterVault(VaultRegistration { chain, epoch, public_key, chain_code, signers, threshold })` (devnet helper).

Idempotency keys kept in the state file: `deposit:<CHAIN>:<txid>:<vout|logIndex|symbol>`,
`unprovable:deposit:ETH:…` (seen, no proof possible), `outbound:<id>`.

## Receipts fallback (`[vault_fallback]`, devnet only)

When `GET /v1/vaults/{chain}` returns `vault: null` and the config has a
`[vault_fallback]`, the daemon rebuilds the view from the events of
`GET /v1/blocks/{h}/receipts` for every block (`src/events.rs`):
`DepositAddressAssigned` → `deposit_owners` and `next_deposit_index`;
`VaultRegistered` → epoch; `WithdrawalQueued` / `OutboundConfirmed` /
`OutboundFailed` → outbound rows. Batching is inferred: an outbound
queued at height `h` is `Batched` once the chain passed the next
multiple of `outbound_batch_interval_blocks`, and that height is its
`batch_id`. The vault key and signer set come from the config (or the
`local:` signer). Gaps: only the last 10,000 blocks of receipts exist,
`fee_estimate` is unknown (0), halted assets are not modelled. The
fallback switches itself off as soon as the primary route answers with
a vault.

## Required upstream changes

All five items found while writing `infra/dev/custody-e2e.sh` were applied
on 2026-09-07:

1. `keel-rpc` serves typed views for `/v1/vaults/{chain}`,
   `/v1/vaults/{chain}/addresses` and `/v1/vaults/outbounds` (the
   `[vault_fallback]` config is no longer needed on devnets).
2. `/v1/blocks/{height}/receipts` carries end-of-block `events`.
3. `Genesis` has `btc_checkpoint`, `eth_checkpoint` and `token_contracts`;
   governance can change them with `SetBtcCheckpoint`, `SetEthCheckpoint`
   and `SetTokenContract` proposals.
4. Bitcoin credits are keyed by `(txid, vout)`; `DepositObservation.index`
   is the output index and the merkle position lives only in the proof.
5. Outbounds settle on the median reported fee and the majority verdict of
   the quorum, not on the closing vote.
