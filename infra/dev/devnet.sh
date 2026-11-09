#!/usr/bin/env bash
# Start a local N-validator Keel devnet (default 4) with --devnet genesis.
# Storage/logs under ${KEEL_DEVNET_DIR:-/tmp/keel-devnet}. `devnet.sh stop`
# kills it. Node i: p2p 3000+i, metrics 4000+i, RPC ${RPC_BASE:-5000}+i.
# Env: N, KEEL_DEVNET_DIR, RPC_BASE, NO_BUILD=1, SNAPSHOT_INTERVAL,
# EPOCH_BLOCKS (--blocks-per-epoch), EXTRA_PEERS (follower seeds the
# validators keep connected, comma separated), DEVNET_IDLE_MS (idle block
# interval, default 5000; e2e runs use 1000).
# `devnet.sh follower <i> <sync-from-rpc> [verify-rpc]` starts node i (a seed
# outside the validator set) that bootstraps from a peer's snapshot.
set -euo pipefail
N=${N:-4}
DIR=${KEEL_DEVNET_DIR:-/tmp/keel-devnet}
RPC_BASE=${RPC_BASE:-5000}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TARGET="${CARGO_TARGET_DIR:-$ROOT/chain/target}"
BIN="$TARGET/release/keel-node"

# A listener on 127.0.0.1:$1 answers a connect; nothing there refuses it.
port_busy() { (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; }
node_ports() { local i=$1; echo "$((3000+i)) $((4000+i)) $((RPC_BASE+i))"; }
# Wait until the given ports are free (up to $WAIT_SECS, default 20). Stopped
# nodes release their p2p, metrics and RPC sockets a moment after the signal;
# starting the next devnet into them leaves validators that cannot bind and a
# chain that never leaves height 0.
wait_ports_free() {
  local t=${WAIT_SECS:-20} p busy
  while (( t > 0 )); do
    busy=""
    for p in "$@"; do port_busy "$p" && busy="$busy $p"; done
    [[ -z "$busy" ]] && return 0
    sleep 1; t=$((t-1))
  done
  echo "devnet: ports still in use:$busy" >&2
  ss -ltnp 2>/dev/null | grep -E ":($(echo "$@" | tr ' ' '|')) " >&2 || true
  return 1
}
# Signal the nodes matching the pattern and wait for them to exit.
stop_nodes() {
  local pat=$1 t=${WAIT_SECS:-20}
  pkill -f "$pat" || true
  while (( t > 0 )) && pgrep -f "$pat" >/dev/null; do sleep 1; t=$((t-1)); done
  pgrep -f "$pat" >/dev/null && { pkill -9 -f "$pat" || true; sleep 1; }
  return 0
}

start_node() {
  local i=$1; shift
  local PORT=$((3000+i))
  local RPC=$((RPC_BASE+i))
  local BOOT=""
  [[ $i -ne 0 ]] && BOOT="--bootstrappers 0@127.0.0.1:3000"
  local EXTRA=""
  [[ -n "${EPOCH_BLOCKS:-}" ]] && EXTRA="$EXTRA --blocks-per-epoch $EPOCH_BLOCKS"
  [[ -n "${DEVNET_IDLE_MS:-}" ]] && EXTRA="$EXTRA --devnet-idle-ms $DEVNET_IDLE_MS"
  [[ -n "${EXTRA_PEERS:-}" ]] && EXTRA="$EXTRA --extra-peers $EXTRA_PEERS"
  # shellcheck disable=SC2086
  nohup "$BIN" --me "$i@$PORT" --participants "$PARTICIPANTS" $BOOT --devnet $EXTRA "$@" \
    --rpc-port "$RPC" --snapshot-interval "${SNAPSHOT_INTERVAL:-200}" \
    --storage-dir "$DIR/$i" >> "$DIR/$i.log" 2>&1 &
  echo "node $i pid $! p2p $PORT rpc $RPC log $DIR/$i.log"
}

PARTICIPANTS=$(seq -s, 0 $((N-1)))
case "${1:-start}" in
  stop)
    stop_nodes "keel-node --me"
    # shellcheck disable=SC2046
    wait_ports_free $(for i in $(seq 0 "$N"); do node_ports "$i"; done) || true
    echo "stopped"; exit 0;;
  restart)
    # restart <i>: bring one validator back on its existing storage.
    i=$2; stop_nodes "keel-node --me $i@"
    # shellcheck disable=SC2046
    wait_ports_free $(node_ports "$i"); start_node "$i"; exit 0;;
  kill)
    i=$2; stop_nodes "keel-node --me $i@"; echo "killed $i"; exit 0;;
  follower)
    # follower <i> <sync-from-rpc> [verify-rpc]: fresh storage, state sync.
    i=$2; FROM=$3; VERIFY=${4:-}
    rm -rf "$DIR/$i"; mkdir -p "$DIR/$i"
    ARGS="--role follower --sync-from $FROM"
    [[ -n "$VERIFY" ]] && ARGS="$ARGS --sync-verify $VERIFY"
    # shellcheck disable=SC2046
    wait_ports_free $(node_ports "$i")
    # shellcheck disable=SC2086
    start_node "$i" $ARGS; exit 0;;
  start)
    [[ "${NO_BUILD:-}" == 1 ]] || (cd "$ROOT/chain" && cargo build --release -p keel-node -p keel-cli)
    rm -rf "$DIR"; mkdir -p "$DIR"
    # shellcheck disable=SC2046
    wait_ports_free $(for i in $(seq 0 $((N-1))); do node_ports "$i"; done) \
      || { echo "devnet: refusing to start into busy ports" >&2; exit 1; }
    for i in $(seq 0 $((N-1))); do start_node "$i"; done
    echo "curl -s http://127.0.0.1:$RPC_BASE/v1/status   # watch height";;
  *) echo "usage: devnet.sh [start|stop|kill <i>|restart <i>|follower <i> <rpc> [verify-rpc]]"; exit 1;;
esac
