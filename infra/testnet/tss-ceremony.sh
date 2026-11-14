#!/usr/bin/env bash
# One host's part of the vault key ceremony, run on every host of the set by
# .github/workflows/tss-ceremony.yml (as the deploy user, sudo for the key
# material). Reads /etc/keelchain/tss-N.env written by the deploy.
#
#   tss-ceremony.sh keygen <epoch>   generate this host's share of the new
#                                    vault key (all hosts must run it within
#                                    the timeout); prints the public key
#   tss-ceremony.sh pubkey           print the public key of the stored share
#   tss-ceremony.sh register <epoch> <chains> <signers>
#                                    host 0: RegisterVault on the chain with
#                                    the stored share's key for each chain
#
# Two shares live under /var/lib/keelchain/tss (root, 0600), encrypted under
# KEEL_TSS_PASSPHRASE: share.enc (CGGMP21, secp256k1: Bitcoin, Ethereum, BSC,
# Tron) and share-sol.enc (FROST, Ed25519: Solana). Previous shares are kept
# as *.prev.
set -euo pipefail
MODE=${1:?keygen|pubkey|register}
BIN=/opt/keelchain/bin; ETC=/etc/keelchain; TSS=/var/lib/keelchain/tss
set -a; . <(sudo cat "$ETC/checkpoint.env"); set +a
N=${HOST_INDEX:-0}
set -a; . <(sudo cat "$ETC/tss-$N.env"); set +a
export KEEL_TSS_PASSPHRASE KEEL_TSS_SECRET

case "$MODE" in
  keygen)
    EPOCH=${2:?epoch}
    sudo install -d -m 700 "$TSS"
    if ! sudo test -f "$TSS/primes.json"; then
      echo "generating Paillier primes (minutes on a small box)"
      sudo -E "$BIN/keel-tss" gen-primes --out "$TSS/primes.json"
      sudo chmod 600 "$TSS/primes.json"
    fi
    sudo test -f "$TSS/share.enc" && sudo mv "$TSS/share.enc" "$TSS/share.enc.prev"
    sudo test -f "$TSS/share-sol.enc" && sudo mv "$TSS/share-sol.enc" "$TSS/share-sol.enc.prev"
    sudo systemctl stop "keel-tss@$N" 2>/dev/null || true
    # Every party listens on its own entry of TSS_PEERS; the ceremony id
    # names the epoch, so a rerun with the same epoch is refused by design.
    # Two ceremonies back to back on the same link: the secp256k1 key, then
    # the Ed25519 one.
    ECDSA=$(sudo -E "$BIN/keel-tss" keygen --index "$TSS_INDEX" --n "$TSS_N" --t "$TSS_T" \
      --peers "$TSS_PEERS" --eid "keel-vault:ALL:epoch:$EPOCH" \
      --primes "$TSS/primes.json" --out "$TSS/share.enc" --timeout-secs 1800)
    sudo chmod 600 "$TSS/share.enc"
    EDDSA=$(sudo -E "$BIN/keel-tss" keygen --scheme eddsa --index "$TSS_INDEX" --n "$TSS_N" --t "$TSS_T" \
      --peers "$TSS_PEERS" --eid "keel-vault:SOL:epoch:$EPOCH" \
      --out "$TSS/share-sol.enc" --timeout-secs 600)
    sudo chmod 600 "$TSS/share-sol.enc"
    sudo systemctl start "keel-tss@$N"
    python3 -c 'import json,sys; a=json.loads(sys.argv[1]); b=json.loads(sys.argv[2]); print(json.dumps({"public_key": a["public_key"], "chain_code": a["chain_code"], "ed25519_public_key": b["public_key"], "ed25519_chain_code": b["chain_code"]}))' "$ECDSA" "$EDDSA"
    ;;
  pubkey)
    A=$(sudo -E "$BIN/keel-tss" pubkey --share "$TSS/share.enc")
    if sudo test -f "$TSS/share-sol.enc"; then
      B=$(sudo -E "$BIN/keel-tss" pubkey --share "$TSS/share-sol.enc")
      python3 -c 'import json,sys; a=json.loads(sys.argv[1]); b=json.loads(sys.argv[2]); print(json.dumps({"public_key": a["public_key"], "chain_code": a["chain_code"], "ed25519_public_key": b["public_key"], "ed25519_chain_code": b["chain_code"]}))' "$A" "$B"
    else
      echo "$A"
    fi
    ;;
  register)
    EPOCH=${2:?epoch}; CHAINS=${3:-BTC,TRON,BSC,SOL}; SIGNERS=${4:?signer addresses, comma separated}
    PK=$(sudo -E "$BIN/keel-tss" pubkey --share "$TSS/share.enc")
    PUB=$(echo "$PK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["public_key"])')
    CC=$(echo "$PK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["chain_code"])')
    for chain in $(echo "$CHAINS" | tr ',' ' '); do
      P=$PUB; C=$CC
      if [ "$chain" = SOL ]; then
        # Solana's vault is the Ed25519 (FROST) key.
        SK=$(sudo -E "$BIN/keel-tss" pubkey --share "$TSS/share-sol.enc")
        P=$(echo "$SK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["public_key"])')
        C=$(echo "$SK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["chain_code"])')
      fi
      sudo bash -c "set -a; . $ETC/observer-$N.env; set +a; $BIN/keel-observer register-vault --config $ETC/observer-$N.toml --chain $chain --epoch $EPOCH --public-key $P --chain-code $C --signers $SIGNERS --threshold $TSS_T"
      sleep 8
    done
    sudo touch /var/lib/keelchain/vaults-registered
    ;;
  *) echo "usage: tss-ceremony.sh keygen <epoch> | pubkey | register <epoch> <chains> <signers>"; exit 2;;
esac
