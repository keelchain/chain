#!/usr/bin/env bash
# State-sync check: 4 validators with short epochs run past several epoch
# boundaries and snapshots, then a fifth node with empty storage joins with
# --sync-from (and --sync-verify against a second validator), catches up and
# reports the same state hash as the validators at the same height. It must
# not hold any block below the snapshot it installed.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-sync-e2e}
export RPC_BASE=${RPC_BASE:-5100}
export SNAPSHOT_INTERVAL=${SNAPSHOT_INTERVAL:-10}
export EPOCH_BLOCKS=${EPOCH_BLOCKS:-10}
export N=4
export EXTRA_PEERS=4
RPC0="http://127.0.0.1:$RPC_BASE"
RPC1="http://127.0.0.1:$((RPC_BASE+1))"
RPC4="http://127.0.0.1:$((RPC_BASE+4))"

cleanup() { "$ROOT/infra/dev/devnet.sh" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
say() { echo "[sync-e2e] $*"; }
height() { curl -sf "$1/v1/status" | python3 -c 'import sys,json;print(json.load(sys.stdin)["height"])' 2>/dev/null || echo 0; }
wait_height() { # rpc target timeout
  local t=$3; while (( t > 0 )); do h=$(height "$1"); (( h >= $2 )) && return 0; sleep 1; t=$((t-1)); done
  echo "timeout waiting for height $2 on $1 (at $(height "$1"))"; return 1
}

"$ROOT/infra/dev/devnet.sh" start
say "running validators past three epochs and a few snapshots (idle blocks every 5 s)"
wait_height "$RPC0" 35 400
META=$(curl -sf "$RPC0/v1/sync/meta")
SNAP_H=$(echo "$META" | python3 -c 'import sys,json;print(json.load(sys.stdin)["height"])')
BOUNDS=$(echo "$META" | python3 -c 'import sys,json;print(len(json.load(sys.stdin)["boundaries"]))')
say "validator 0 serves snapshot at $SNAP_H with $BOUNDS epoch boundaries"
(( SNAP_H >= 10 )) || { say "no snapshot yet"; exit 1; }
(( BOUNDS >= 1 )) || { say "no epoch boundary recorded"; exit 1; }

say "starting follower 4 from validator 0's snapshot, verified against validator 1"
"$ROOT/infra/dev/devnet.sh" follower 4 "$RPC0" "$RPC1"
sleep 3
grep -q "state sync: installed snapshot" "$KEEL_DEVNET_DIR/4.log" || { say "follower did not install a snapshot:"; tail -20 "$KEEL_DEVNET_DIR/4.log"; exit 1; }
say "waiting for the follower to catch up"
TARGET=$(( $(height "$RPC0") + 4 ))
wait_height "$RPC4" "$TARGET" 400

H=$(height "$RPC4")
F=$(curl -sf "$RPC4/v1/status" | python3 -c 'import sys,json;print(json.load(sys.stdin)["state_hash"])')
V=$(curl -sf "$RPC0/v1/blocks/$H" | python3 -c 'import sys,json;print(json.load(sys.stdin)["state_hash"])')
[[ "$F" == "$V" ]] || { say "hash mismatch at $H: follower=$F validator=$V"; exit 1; }
say "follower at $H agrees with validator 0 ($F)"

# The follower never held blocks below its snapshot.
BELOW=$((SNAP_H - 1))
if curl -sf "$RPC4/v1/blocks/$BELOW" >/dev/null; then say "follower serves block $BELOW, below its snapshot"; exit 1; fi
say "follower has no block below $SNAP_H, as expected"
say "PASS"
