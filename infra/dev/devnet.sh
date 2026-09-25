#!/usr/bin/env bash
# Start a local N-validator Keel devnet (default 4) with --devnet genesis.
# Storage/logs under ${KEEL_DEVNET_DIR:-/tmp/keel-devnet}. `devnet.sh stop`
# kills it. Node i: p2p 3000+i, metrics 4000+i, RPC ${RPC_BASE:-5000}+i.
# Env: N, KEEL_DEVNET_DIR, RPC_BASE, NO_BUILD=1, SNAPSHOT_INTERVAL.
set -euo pipefail
N=${N:-4}
DIR=${KEEL_DEVNET_DIR:-/tmp/keel-devnet}
RPC_BASE=${RPC_BASE:-5000}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TARGET="${CARGO_TARGET_DIR:-$ROOT/chain/target}"
BIN="$TARGET/release/keel-node"

start_node() {
  local i=$1
  local PORT=$((3000+i))
  local RPC=$((RPC_BASE+i))
  local BOOT=""
  [[ $i -ne 0 ]] && BOOT="--bootstrappers 0@127.0.0.1:3000"
  # shellcheck disable=SC2086
  nohup "$BIN" --me "$i@$PORT" --participants "$PARTICIPANTS" $BOOT --devnet \
    --rpc-port "$RPC" --snapshot-interval "${SNAPSHOT_INTERVAL:-200}" \
    --storage-dir "$DIR/$i" >> "$DIR/$i.log" 2>&1 &
  echo "validator $i pid $! p2p $PORT rpc $RPC log $DIR/$i.log"
}

PARTICIPANTS=$(seq -s, 0 $((N-1)))
case "${1:-start}" in
  stop)
    pkill -f "keel-node --me" || true
    echo "stopped"; exit 0;;
  restart)
    # restart <i>: bring one validator back on its existing storage.
    i=$2; pkill -f "keel-node --me $i@" || true; sleep 1; start_node "$i"; exit 0;;
  kill)
    i=$2; pkill -f "keel-node --me $i@" || true; echo "killed $i"; exit 0;;
  start)
    [[ "${NO_BUILD:-}" == 1 ]] || (cd "$ROOT/chain" && cargo build --release -p keel-node -p keel-cli)
    rm -rf "$DIR"; mkdir -p "$DIR"
    for i in $(seq 0 $((N-1))); do start_node "$i"; done
    echo "curl -s http://127.0.0.1:$RPC_BASE/v1/status   # watch height";;
  *) echo "usage: devnet.sh [start|stop|kill <i>|restart <i>]"; exit 1;;
esac
