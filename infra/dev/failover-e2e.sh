#!/usr/bin/env bash
# Failover check on a 4-validator devnet (BFT: n = 3f + 1, so four validators
# tolerate one fault and three tolerate none): with one validator down the
# chain keeps finalizing; with two down it halts; when they return it resumes
# and every node agrees on the state hash.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-failover}
export RPC_BASE=${RPC_BASE:-5200}
export DEVNET_IDLE_MS=${DEVNET_IDLE_MS:-1000}
export N=4
RPC0="http://127.0.0.1:$RPC_BASE"; RPC1="http://127.0.0.1:$((RPC_BASE+1))"; RPC2="http://127.0.0.1:$((RPC_BASE+2))"; RPC3="http://127.0.0.1:$((RPC_BASE+3))"
cleanup() { "$ROOT/infra/dev/devnet.sh" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
say() { echo "[failover-e2e] $*"; }
height() { curl -sf "$1/v1/status" | python3 -c 'import sys,json;print(json.load(sys.stdin)["height"])' 2>/dev/null || echo 0; }
hash_of() { curl -sf "$1/v1/blocks/$2" | python3 -c 'import sys,json;print(json.load(sys.stdin)["state_hash"])' 2>/dev/null || echo none; }
wait_height() { local t=$3; while (( t > 0 )); do h=$(height "$1"); (( h >= $2 )) && return 0; sleep 1; t=$((t-1)); done; say "timeout waiting for height $2 on $1 (at $(height "$1"))"; return 1; }

NO_BUILD=${NO_BUILD:-} "$ROOT/infra/dev/devnet.sh" start
say "four validators; waiting for height 6"
for i in 0 1 2 3; do wait_height "http://127.0.0.1:$((RPC_BASE+i))" 6 90; done

say "kill validator 3: three of four keep finalizing"
"$ROOT/infra/dev/devnet.sh" kill 3
H=$(height "$RPC0"); wait_height "$RPC0" $((H+5)) 60
say "height advanced from $H to $(height "$RPC0") with one validator down"

say "kill validator 2: two of four cannot finalize"
"$ROOT/infra/dev/devnet.sh" kill 2
sleep 3
H=$(height "$RPC0"); sleep 15
H2=$(height "$RPC0")
(( H2 - H <= 1 )) || { say "chain advanced from $H to $H2 with a single validator; quorum is broken"; exit 1; }
say "height held at $H2 with two validators down"

say "restart validators 2 and 3: finalization resumes"
NO_BUILD=1 "$ROOT/infra/dev/devnet.sh" restart 2
NO_BUILD=1 "$ROOT/infra/dev/devnet.sh" restart 3
wait_height "$RPC0" $((H2+5)) 120
T=$(height "$RPC0")
wait_height "$RPC2" "$T" 120; wait_height "$RPC3" "$T" 120
A=$(hash_of "$RPC0" "$T"); B=$(hash_of "$RPC1" "$T"); C=$(hash_of "$RPC2" "$T"); D=$(hash_of "$RPC3" "$T")
[[ "$A" != none && "$A" == "$B" && "$B" == "$C" && "$C" == "$D" ]] || { say "state hash mismatch at $T: $A / $B / $C / $D"; exit 1; }
say "all four agree at height $T ($A)"
say "PASS"
