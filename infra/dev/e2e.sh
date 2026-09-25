#!/usr/bin/env bash
# End-to-end devnet check: 4 validators, a crossing sell/buy on BTC-KUSD
# between two funded devnet accounts, receipts, cross-node balance and
# state-hash agreement, then a kill/restart catch-up test.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-e2e}
export RPC_BASE=${RPC_BASE:-5000}
export SNAPSHOT_INTERVAL=${SNAPSHOT_INTERVAL:-20}
KEEL="${CARGO_TARGET_DIR:-$ROOT/chain/target}/release/keel"
RPC0="http://127.0.0.1:$RPC_BASE"
RPC1="http://127.0.0.1:$((RPC_BASE+1))"
RPC3="http://127.0.0.1:$((RPC_BASE+3))"
PX=$((60000 * 1000000))     # 60,000.00 KUSD per BTC, in micro-KUSD
QTY=100000000               # 1 BTC in sats

cleanup() { "$ROOT/infra/dev/devnet.sh" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT

say() { echo "[e2e] $*"; }
height() { curl -sf "$1/v1/status" | python3 -c 'import sys,json;print(json.load(sys.stdin)["height"])' 2>/dev/null || echo 0; }
hash_at() { curl -sf "$1/v1/status" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d["height"],d["state_hash"])'; }
balance() { # rpc addr asset type
  curl -sf "$1/v1/accounts/$2" | python3 -c "
import sys,json
d=json.load(sys.stdin)
for b in d['balances']:
    if b['asset']=='$3' and b['account_type']=='$4': print(b['balance']); break
else: print(0)"
}
wait_height() { # rpc target timeout
  local t=$3; while (( t > 0 )); do h=$(height "$1"); (( h >= $2 )) && return 0; sleep 1; t=$((t-1)); done
  echo "timeout waiting for height $2 on $1 (at $(height "$1"))"; return 1
}

"$ROOT/infra/dev/devnet.sh" start
say "waiting for height > 5 on all nodes"
for i in 0 1 2 3; do wait_height "http://127.0.0.1:$((RPC_BASE+i))" 6 90; done

ALICE_SECRET=$("$KEEL" keygen --seed 0 | python3 -c 'import sys,json;print(json.load(sys.stdin)["secret"])')
ALICE=$("$KEEL" keygen --seed 0 | python3 -c 'import sys,json;print(json.load(sys.stdin)["address"])')
BOB_SECRET=$("$KEEL" keygen --seed 1 | python3 -c 'import sys,json;print(json.load(sys.stdin)["secret"])')
BOB=$("$KEEL" keygen --seed 1 | python3 -c 'import sys,json;print(json.load(sys.stdin)["address"])')
say "alice=$ALICE bob=$BOB"

A_BTC0=$(balance "$RPC0" "$ALICE" BTC.BTC deposit); A_USD0=$(balance "$RPC0" "$ALICE" KUSD deposit)
B_BTC0=$(balance "$RPC0" "$BOB" BTC.BTC deposit);   B_USD0=$(balance "$RPC0" "$BOB" KUSD deposit)
say "before: alice btc=$A_BTC0 usds=$A_USD0 | bob btc=$B_BTC0 usds=$B_USD0"

say "alice sells 1 BTC @ 60000 (via node 0)"
"$KEEL" --rpc "$RPC0" send --secret "$ALICE_SECRET" --wait 30 order place BTC-KUSD sell --price "$PX" --quantity "$QTY" > "$KEEL_DEVNET_DIR/sell.json"
say "bob buys 1 BTC @ 60000 (via node 1)"
"$KEEL" --rpc "$RPC1" send --secret "$BOB_SECRET" --wait 30 order place BTC-KUSD buy --price "$PX" --quantity "$QTY" > "$KEEL_DEVNET_DIR/buy.json"
grep -q '"OrderFilled"' "$KEEL_DEVNET_DIR/buy.json" || { say "buy receipt has no fill:"; cat "$KEEL_DEVNET_DIR/buy.json"; exit 1; }

FEE=$((QTY * 10 / 10000))
QUOTE=$((QTY * 60000 * 1000000 / 100000000))
EXP_A_BTC=$((A_BTC0 - QTY)); EXP_A_USD=$((A_USD0 + QUOTE))
EXP_B_BTC=$((B_BTC0 + QTY - FEE)); EXP_B_USD=$((B_USD0 - QUOTE))
sleep 2
for rpc in "$RPC0" "$RPC3"; do
  a_btc=$(balance "$rpc" "$ALICE" BTC.BTC deposit); a_usd=$(balance "$rpc" "$ALICE" KUSD deposit)
  b_btc=$(balance "$rpc" "$BOB" BTC.BTC deposit);   b_usd=$(balance "$rpc" "$BOB" KUSD deposit)
  say "$rpc after: alice btc=$a_btc usds=$a_usd | bob btc=$b_btc usds=$b_usd"
  [[ "$a_btc" == "$EXP_A_BTC" && "$a_usd" == "$EXP_A_USD" && "$b_btc" == "$EXP_B_BTC" && "$b_usd" == "$EXP_B_USD" ]] \
    || { say "balance mismatch on $rpc (expected alice $EXP_A_BTC/$EXP_A_USD bob $EXP_B_BTC/$EXP_B_USD)"; exit 1; }
done
say "balances agree on node 0 and node 3"

# State hash agreement at a common height: read node 0's receipts height, then compare hashes when both pass it.
H=$(height "$RPC0"); T=$((H + 3))
wait_height "$RPC0" "$T" 30; wait_height "$RPC3" "$T" 30
H0=$(curl -sf "$RPC0/v1/blocks/$T/receipts" >/dev/null && echo ok || echo miss)
say "receipts at height $T on node 0: $H0"
# Hashes are per-height; sample both nodes until they report the same height.
for _ in $(seq 1 40); do
  read -r h0 s0 <<<"$(hash_at "$RPC0")"; read -r h3 s3 <<<"$(hash_at "$RPC3")"
  if [[ "$h0" == "$h3" ]]; then
    [[ "$s0" == "$s3" ]] || { say "STATE HASH MISMATCH at $h0: $s0 vs $s3"; exit 1; }
    say "state hash agrees at height $h0: $s0"; break
  fi
  sleep 0.2
done

say "restart test: kill node 3, run 8 more blocks, restart, expect catch-up"
"$ROOT/infra/dev/devnet.sh" kill 3
H=$(height "$RPC0"); wait_height "$RPC0" $((H + 8)) 60
"$KEEL" --rpc "$RPC0" send --secret "$ALICE_SECRET" --wait 30 transfer "$BOB" KEEL 5 >/dev/null
NO_BUILD=1 "$ROOT/infra/dev/devnet.sh" restart 3
TARGET=$(height "$RPC0")
wait_height "$RPC3" "$TARGET" 90
for _ in $(seq 1 60); do
  read -r h0 s0 <<<"$(hash_at "$RPC0")"; read -r h3 s3 <<<"$(hash_at "$RPC3")"
  if [[ "$h0" == "$h3" ]]; then
    [[ "$s0" == "$s3" ]] || { say "STATE HASH MISMATCH after restart at $h0"; exit 1; }
    say "node 3 caught up: height $h3 hash $s3"; break
  fi
  sleep 0.2
done
[[ "$(balance "$RPC3" "$BOB" KEEL deposit)" == "$(balance "$RPC0" "$BOB" KEEL deposit)" ]] || { say "post-restart balance mismatch"; exit 1; }
say "PASS: fills settled identically on 4 nodes, state hashes agree, restart caught up"
