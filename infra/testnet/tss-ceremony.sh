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
# The share lives at /var/lib/keelchain/tss/share.enc (root, 0600), encrypted
# under KEEL_TSS_PASSPHRASE. A previous share is kept as share.enc.prev.
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
    sudo systemctl stop "keel-tss@$N" 2>/dev/null || true
    # Every party listens on its own entry of TSS_PEERS; the ceremony id
    # names the epoch, so a rerun with the same epoch is refused by design.
    sudo -E "$BIN/keel-tss" keygen --index "$TSS_INDEX" --n "$TSS_N" --t "$TSS_T" \
      --peers "$TSS_PEERS" --eid "keel-vault:ALL:epoch:$EPOCH" \
      --primes "$TSS/primes.json" --out "$TSS/share.enc" --timeout-secs 1800
    sudo chmod 600 "$TSS/share.enc"
    sudo systemctl start "keel-tss@$N"
    ;;
  pubkey)
    sudo -E "$BIN/keel-tss" pubkey --share "$TSS/share.enc"
    ;;
  register)
    EPOCH=${2:?epoch}; CHAINS=${3:-BTC,TRON}; SIGNERS=${4:?signer addresses, comma separated}
    PK=$(sudo -E "$BIN/keel-tss" pubkey --share "$TSS/share.enc")
    PUB=$(echo "$PK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["public_key"])')
    CC=$(echo "$PK" | python3 -c 'import json,sys; print(json.load(sys.stdin)["chain_code"])')
    for chain in $(echo "$CHAINS" | tr ',' ' '); do
      sudo bash -c "set -a; . $ETC/observer-$N.env; set +a; $BIN/keel-observer register-vault --config $ETC/observer-$N.toml --chain $chain --epoch $EPOCH --public-key $PUB --chain-code $CC --signers $SIGNERS --threshold $TSS_T"
      sleep 8
    done
    sudo touch /var/lib/keelchain/vaults-registered
    ;;
  *) echo "usage: tss-ceremony.sh keygen <epoch> | pubkey | register <epoch> <chains> <signers>"; exit 2;;
esac
