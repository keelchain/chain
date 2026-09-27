# keel-tss

Threshold signer of one observer for the KEEL vaults (docs/plan.md §4).
`t`-of-`n` ECDSA on secp256k1 with CGGMP21 and non-hardened BIP32/SLIP-10
child derivation, so the deposit addresses the chain derives from the vault
key (`keel-chains::hd`) are signed without any party ever holding the whole
key or a child key.

```text
keel-tss gen-primes --out primes.json                     # once per host, slow
keel-tss keygen  --index 0 --n 4 --t 3 --eid keel-vault:BTC:epoch:1 \
                --peers h0:7000,h1:7000,h2:7000,h3:7000 --out share.enc --primes primes.json
keel-tss pubkey  --share share.enc
keel-tss serve   --share share.enc --peers h0:7000,... --http 127.0.0.1:7100 --policy-rpc http://127.0.0.1:5100
```

Environment: `KEEL_TSS_PASSPHRASE` (share file), `KEEL_TSS_SECRET` (hex, the
HMAC key of the signer set's transport).

## What is real and what is not

| Piece | Status |
|---|---|
| DKG + auxiliary info (`cggmp21` 0.6, `hd-wallet`, `hd-slip10`, `SecurityLevel128`) | Real. `ecdsa::keygen` runs `cggmp21::keygen(...).hd_wallet(true)` (the parties agree on a chain code) and then `aux_info_gen` (Paillier moduli, ring-Pedersen parameters) over the same link. |
| Signing with derivation | Real. `ecdsa::sign` runs the CGGMP21 signing protocol with exactly `t` signers; the derivation path is passed as `set_derivation_path`, so the additive tweak is folded into the signature and no party learns a child secret. `s` is normalized (low-s); the recovery id is computed against the derived child key and checked to recover it. |
| Agreement with the chain's derivation | Tested: `tests/ecdsa.rs` derives `keel_chains::hd::deposit_path(chain, i)` from `(vault_public_key, chain_code)` and verifies the threshold signature under that child key (`keel_chains::eth::recovery_id` agrees too). |
| Share encryption (`store`) | Real: PBKDF2-HMAC-SHA256 (200,000 iterations, 16-byte salt) → ChaCha20-Poly1305 with the JSON header as associated data. |
| Transport (`transport::TcpTransport`) | Real but minimal: one JSON line per message over plain TCP, HMAC-SHA256 over a secret shared by the whole set, replay filter of the last 65,536 frames. Authenticates membership, not the sender: run it on a private network or through a TLS tunnel. |
| `serve` HTTP endpoint and signing policy | Real. `POST /sign` takes the digest, the derivation path and a `context` describing the transaction (`keel_chains::policy::SignContext`). With `--policy-rpc <node>` the coordinator checks the context before announcing and every joiner checks it before taking part: the digest must be the signing hash of the described transaction, the transaction must pay open outbounds of the named batch on the chain, and the signing key must be the vault key the context claims (the spent output on Bitcoin, the owner on Tron). Without the flag any digest is signed (devnet only). Bind `--http` to loopback either way. |
| Presign pool | Not implemented; every request runs the full (3+1)-round protocol (seconds on a LAN). |
| Resharing / t,n changes | Not possible with CGGMP21 by design; a new observer set means a new ceremony and a new vault epoch (plan §4). |
| FROST Ed25519 (`--features frost`) | Library only (`frost::keygen`, `frost::sign`, tested 2-of-3). Not wired into `serve`; Solana comes later. |
| Constant time | `cggmp21` does not claim constant-time arithmetic; run the signer on a dedicated host. |
| `local:` dev signer | Lives in `keel-observer` (`tss_url = "local:<hex seed>"`): one BIP32 master key in one process. Same derivation, same addresses, no threshold. Devnet only. |

## Keygen ceremony

Every party of the new observer set runs `keygen` with identical `--n`,
`--t`, `--eid` and the same `--peers` list in index order; party `i`
listens on `peers[i]` (or `--listen` when behind NAT). The execution id
must be unique per ceremony (`keel-vault:<CHAIN>:epoch:<E>` is the
convention); reusing one across ceremonies is unsafe.

1. `gen-primes` on every host beforehand. Generating two 1536-bit safe
   primes takes minutes; the file is per party and can be reused across
   ceremonies of that host.
2. Start all `n` `keygen` processes within the `--timeout-secs` window
   (default 1800 s). The DKG needs every party; a missing one fails the
   ceremony for all.
3. Each process writes its encrypted share and prints `public_key`
   (compressed, 33 bytes hex) and `chain_code`. Compare the printed values
   out of band: they must be identical on every host.
4. One observer submits `RegisterVault { chain, epoch, public_key,
   chain_code, signers, threshold }` on the Keelchain (`keel-observer
   register-vault --public-key … --chain-code …`). From that height the
   chain derives deposit addresses from the key, and `keel-observer
   addresses` prints them.

The same share can serve several chains: the path starts with the SLIP-44
coin index (`m/0/i` Bitcoin, `m/60/i` Ethereum, `m/195/i` Tron), so one
ceremony per epoch is enough for the secp256k1 chains. Registering the
same key for each chain is what the devnet does.

## Serving signatures

`serve` loads the share, joins the TCP mesh and listens on `--http` for

```json
POST /sign  {"digest": "<32 bytes hex>", "path": [0, 17], "context": {"chain": "BTC", "batch_id": 7, "raw_tx": "<hex>", "input": 0, "prevout_value": 100000, "prevout_pubkey": "<33 bytes hex>", "network": "signet"}}
→ 200       {"r": "<hex>", "s": "<hex>", "v": 0}
→ 403       {"error": "policy: ..."}     (context missing or not matching the chain)
→ 400/500   {"error": "..."}
```

Contexts per chain: `BTC` (unsigned raw tx, the input, its previous output's
value and key, the network), `ETH` (the EIP-1559 fields), `TRON` (the
`raw_data` protobuf). The observer daemon fills them in; a client that runs
its own signer for its own vault gets the same check for free.

The receiving party coordinates: it picks the signer subset (`--signers`,
default itself plus the lowest other indexes, exactly `t` parties), sends
a `control` announcement `{session, digest, path, signers}` to each of
them, and every party runs the signing protocol for that session. A party
only joins announcements that list both itself and the announcer.
Sessions run one at a time per process; announcements that arrive during
a session are queued. Two parties coordinating simultaneously will make
one of the sessions wait for the other (and time out after
`--timeout-secs`, default 120 s), so an observer set should let one
daemon lead per batch — which is what `keel-observer` does
(`batch_id % signers.len()`).

## Share files

```json
{
  "header": { "version": 1, "kdf": "pbkdf2-hmac-sha256", "salt": "…", "iterations": 200000,
              "index": 0, "n": 4, "t": 3, "public_key": "02…", "chain_code": "…" },
  "nonce": "…", "ciphertext": "…"
}
```

The header is readable without the passphrase (`serve` uses it for `n`,
`t` and the party index) but not trustworthy until the share decrypts:
it is the AEAD's associated data, and the decrypted share's index must
match `header.index`. Wrong passphrase and any header edit both fail as
`StoreError::Decrypt`/`Invalid`. Files are written to `<out>.tmp` and
renamed.

## HD derivation

The CGGMP21 shared key with `hd_wallet(true)` is a BIP32 parent
`(public_key, chain_code)`. Children are `parent + I_L·G` with
`I = HMAC-SHA512(chain_code, ser_P(parent) || ser32(i))`, exactly what
`keel_chains::hd::child_pubkey` computes from the registered vault key.
Only non-hardened indexes (`< 2^31`) exist; `hd::deposit_path` splits a
larger deposit index across two levels. Index 0 is never handed to a
user and is the vault's hot/change address.

## Tests

`cargo test -p keel-tss` runs a 3-of-4 keygen, signs with two different
subsets and paths, checks the derived keys against `keel-chains`, and
round-trips the encrypted store. `tests/fixtures/primes.json` holds four
pregenerated prime sets so the aux-info round does not spend minutes on
prime search; delete it to exercise generation. `--features frost` adds
the FROST test.
