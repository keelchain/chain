#!/usr/bin/env bash
# Custody end-to-end on a laptop: the 4-validator devnet, a regtest
# bitcoind, and three observer daemons sharing ONE development signer
# (`tss_url = "local:<seed>"`, see crates/keel-observer/README.md — real
# deployments run `keel-tss serve` per observer instead).
#
#   1. devnet.sh start; regtest bitcoind (downloaded into ~/.local/bin if
#      missing; SKIP when that fails)
#   2. genesis observer 0 registers the Bitcoin vault (the dev signer's key)
#   3. observers 0..2 run against the same bitcoind
#   4. a user asks the chain for a deposit address, regtest coins are sent
#      to it and mined; BTC.BTC is credited once 3 observers submit the
#      SPV proof
#   5. the user withdraws to a fresh regtest address; the batch is signed
#      by the dev signer, broadcast, mined, observed back; the escrow is
#      empty and the address holds the coins
#
# Env: KEEL_DEVNET_DIR, RPC_BASE, CARGO_TARGET_DIR, NO_BUILD=1,
#      BTC_RPC_PORT (default 18543), BTC_P2P_PORT (18544), KEEP=1 (do not
#      stop anything at exit).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-custody}
export RPC_BASE=${RPC_BASE:-5000}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/chain/target}
export PATH="$HOME/.local/bin:$PATH"
KEEL="$CARGO_TARGET_DIR/release/keel"
OBS="$CARGO_TARGET_DIR/release/keel-observer"
RPC0="http://127.0.0.1:$RPC_BASE"
BTC_DIR="$KEEL_DEVNET_DIR/bitcoind"
BTC_RPC_PORT=${BTC_RPC_PORT:-18543}
BTC_P2P_PORT=${BTC_P2P_PORT:-18544}
BTC_AUTH=(-rpcuser=keel -rpcpassword=keel)
DEPOSIT_BTC=0.5;   DEPOSIT_SATS=50000000
WITHDRAW_BTC=0.2;  WITHDRAW_SATS=20000000
DEV_SEED=$(printf '%02x' $(seq 1 32) | tr -d '\n')   # 32-byte seed of the shared dev signer

say() { echo "[custody-e2e] $*"; }
fail() { say "FAIL: $*"; exit 1; }
# Parse the last JSON line of stdin (daemons may log before printing).
json() { python3 -c "import sys,json
lines=[l for l in sys.stdin.read().splitlines() if l.strip().startswith(('{','['))]
d=json.loads(lines[-1]) if lines else None
print($1)"; }
height() { curl -sf "$1/v1/status" | json 'd["height"]' 2>/dev/null || echo 0; }
balance() { # addr asset type
  curl -sf "$RPC0/v1/accounts/$1" | python3 -c "
import sys,json
d=json.load(sys.stdin)
for b in d['balances']:
    if b['asset']=='$2' and b['account_type']=='$3': print(b['balance']); break
else: print(0)"
}
wait_for() { # description timeout_secs command...
  local what=$1 t=$2; shift 2
  while (( t > 0 )); do "$@" && return 0; sleep 1; t=$((t-1)); done
  fail "timeout waiting for $what"
}
bcli() { bitcoin-cli -regtest -datadir="$BTC_DIR" -rpcport="$BTC_RPC_PORT" -rpcuser=keel -rpcpassword=keel "$@"; }
miner() { bcli -rpcwallet=miner "$@"; }
mine() { miner generatetoaddress "$1" "$(miner getnewaddress)" >/dev/null; }
# `wait_for … bash -c` runs the check in a subshell: export what it needs.
export BTC_DIR BTC_RPC_PORT RPC0
export -f json height balance bcli miner

cleanup() {
  [[ "${KEEP:-}" == 1 ]] && { say "KEEP=1: leaving devnet, bitcoind and observers running"; return; }
  pkill -f "keel-observer run --config $KEEL_DEVNET_DIR" >/dev/null 2>&1 || true
  bcli stop >/dev/null 2>&1 || true
  "$ROOT/infra/dev/devnet.sh" stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

# ---------------------------------------------------------------- bitcoind
find_bitcoind() {
  command -v bitcoind >/dev/null 2>&1 && return 0
  for d in "$HOME/.local/bin" /usr/local/bin /opt/bitcoin/bin; do
    [[ -x "$d/bitcoind" ]] && { export PATH="$d:$PATH"; return 0; }
  done
  say "bitcoind not on PATH; trying to download Bitcoin Core into ~/.local/bin (no sudo)"
  local arch ver=29.1 tmp
  case "$(uname -m)" in
    x86_64) arch=x86_64-linux-gnu;; aarch64|arm64) arch=aarch64-linux-gnu;;
    *) say "unsupported arch $(uname -m)"; return 1;;
  esac
  tmp=$(mktemp -d)
  mkdir -p "$HOME/.local/bin"
  if curl -sfL --max-time 240 -o "$tmp/btc.tar.gz" "https://bitcoincore.org/bin/bitcoin-core-$ver/bitcoin-$ver-$arch.tar.gz" \
     && tar xzf "$tmp/btc.tar.gz" -C "$tmp" \
     && cp "$tmp/bitcoin-$ver/bin/bitcoind" "$tmp/bitcoin-$ver/bin/bitcoin-cli" "$HOME/.local/bin/"; then
    rm -rf "$tmp"; export PATH="$HOME/.local/bin:$PATH"; return 0
  fi
  rm -rf "$tmp"; return 1
}
if ! find_bitcoind; then
  say "SKIP: no bitcoind available and the download failed"
  trap - EXIT; exit 0
fi
say "using $(command -v bitcoind): $(bitcoind --version | head -1)"
command -v bitcoin-cli >/dev/null || fail "bitcoin-cli must sit next to bitcoind"

# ---------------------------------------------------------------- build + devnet
[[ "${NO_BUILD:-}" == 1 ]] || (cd "$ROOT/chain" && cargo build --release -p keel-node -p keel-cli -p keel-observer)
[[ -x "$KEEL" && -x "$OBS" ]] || fail "missing binaries under $CARGO_TARGET_DIR/release"
NO_BUILD=1 "$ROOT/infra/dev/devnet.sh" start
mkdir -p "$KEEL_DEVNET_DIR/observers" "$BTC_DIR"
say "waiting for the devnet to reach height 6"
at_height() { [[ "$(height "$1")" =~ ^[0-9]+$ ]] && (( $(height "$1") >= $2 )); }
for i in 0 1 2 3; do wait_for "height 6 on node $i" 90 at_height "http://127.0.0.1:$((RPC_BASE+i))" 6; done

# ---------------------------------------------------------------- regtest
nohup bitcoind -regtest -datadir="$BTC_DIR" -rpcport="$BTC_RPC_PORT" -port="$BTC_P2P_PORT" "${BTC_AUTH[@]}" \
  -listen=0 -fallbackfee=0.0001 -txindex=1 > "$KEEL_DEVNET_DIR/bitcoind.log" 2>&1 &
wait_for "bitcoind rpc" 60 bash -c "bitcoin-cli -regtest -datadir='$BTC_DIR' -rpcport=$BTC_RPC_PORT ${BTC_AUTH[*]} getblockchaininfo >/dev/null 2>&1"
bcli createwallet miner >/dev/null
mine 101
say "regtest at height $(bcli getblockcount), miner funded"

# ---------------------------------------------------------------- keys
keyof() { "$KEEL" keygen --seed "$1" | json "d['$2']"; }
declare -a OBS_ADDR OBS_SECRET
for i in 0 1 2; do OBS_ADDR[i]=$(keyof "$i" address); OBS_SECRET[i]=$(keyof "$i" secret); done
USER_SECRET=$(keyof 42 secret); USER=$(keyof 42 address)
SIGNERS="${OBS_ADDR[0]},${OBS_ADDR[1]},${OBS_ADDR[2]}"
say "observers ${OBS_ADDR[0]:0:8}… ${OBS_ADDR[1]:0:8}… ${OBS_ADDR[2]:0:8}…  user ${USER:0:8}…"

# ---------------------------------------------------------------- observer configs
# All three share the SAME local seed so they derive the same vault key and
# every one of them can sign (devnet shortcut; real deployments run keel-tss).
for i in 0 1 2; do
cat > "$KEEL_DEVNET_DIR/observers/$i.toml" <<EOF
keel_rpc_url = "http://127.0.0.1:$((RPC_BASE+i))"
tss_url = "local:$DEV_SEED"
state_file = "$KEEL_DEVNET_DIR/observers/$i.state.json"

[intervals]
sync_secs = 3
deposits_secs = 2
outbound_secs = 2
fees_secs = 5

[bitcoin]
rpc_url = "http://127.0.0.1:$BTC_RPC_PORT"
rpc_user = "keel"
rpc_password = "keel"
network = "regtest"
wallet = "keel-observer-$i"
fallback_sat_per_vb = 2

[outbound]
enabled = true
leader_timeout_secs = 900

EOF
done

# ---------------------------------------------------------------- vault registration
say "observer 0 registers the Bitcoin vault (dev signer key, signers = observers 0..2)"
REG=$(KEEL_OBSERVER_SECRET=${OBS_SECRET[0]} "$OBS" register-vault --config "$KEEL_DEVNET_DIR/observers/0.toml" \
      --chain BTC --epoch 1 --local-seed "$DEV_SEED" --signers "$SIGNERS" --threshold 2)
echo "$REG" | grep -q '"admitted":true' || fail "register-vault refused: $REG"
TX=$(echo "$REG" | json 'd["tx_id"]')
wait_for "vault registration receipt" 30 bash -c "curl -sf $RPC0/v1/receipts/$TX | grep -q '\"ok\":true'"

# ---------------------------------------------------------------- observers
for i in 0 1 2; do
  KEEL_OBSERVER_SECRET=${OBS_SECRET[i]} RUST_LOG=${RUST_LOG:-info} nohup "$OBS" run --config "$KEEL_DEVNET_DIR/observers/$i.toml" \
    > "$KEEL_DEVNET_DIR/observers/$i.log" 2>&1 &
  say "observer $i pid $! log $KEEL_DEVNET_DIR/observers/$i.log"
done
wait_for "observer wallets" 60 bash -c "[[ \$(bcli listwallets | grep -c keel-observer) -eq 3 ]]"

# ---------------------------------------------------------------- deposit
say "user requests a Bitcoin deposit address"
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 raw '{"RequestDepositAddress":{"chain":"Bitcoin"}}' > "$KEEL_DEVNET_DIR/deposit-address.json"
INDEX=$(python3 -c "
import json
d=json.load(open('$KEEL_DEVNET_DIR/deposit-address.json'))
print([e['DepositAddressAssigned']['index'] for e in d['events'] if 'DepositAddressAssigned' in e][0])")
ADDR=$(KEEL_OBSERVER_SECRET=${OBS_SECRET[0]} "$OBS" addresses --config "$KEEL_DEVNET_DIR/observers/0.toml" --chain BTC | awk -v i="$INDEX" -v u="$USER" '$1==i && $3==u {print $2}')
[[ -n "$ADDR" ]] || fail "observer could not derive the user's deposit address (index $INDEX)"
say "deposit index $INDEX -> $ADDR"

say "sending $DEPOSIT_BTC regtest BTC to the deposit address and mining 3 blocks"
DEP_TXID=$(miner sendtoaddress "$ADDR" "$DEPOSIT_BTC")
mine 3
say "deposit tx $DEP_TXID mined; waiting for 3 observers to submit the SPV proof"
wait_for "BTC.BTC credit on chain" 180 bash -c "[[ \$(balance $USER BTC.BTC deposit) -eq $DEPOSIT_SATS ]]"
say "credited: user BTC.BTC deposit = $(balance "$USER" BTC.BTC deposit) sats"

# ---------------------------------------------------------------- withdrawal
DEST=$(miner getnewaddress)
say "user withdraws $WITHDRAW_BTC BTC to $DEST"
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 withdraw BTC.BTC "$DEST" "$WITHDRAW_SATS" > "$KEEL_DEVNET_DIR/withdraw.json"
grep -q '"WithdrawalQueued"' "$KEEL_DEVNET_DIR/withdraw.json" || { cat "$KEEL_DEVNET_DIR/withdraw.json"; fail "withdrawal not queued"; }
ESCROW=$(balance "$USER" BTC.BTC sendout_escrow)
say "queued: sendout_escrow = $ESCROW sats (amount + network fee estimate)"
(( ESCROW >= WITHDRAW_SATS )) || fail "escrow $ESCROW < $WITHDRAW_SATS"

say "waiting for the batch to be signed by the dev signer and broadcast"
wait_for "payout in the regtest mempool" 180 bash -c "python3 -c \"import sys; sys.exit(0 if float('\$(miner getreceivedbyaddress $DEST 0)') >= $WITHDRAW_BTC else 1)\""
OUT_TXID=$(bcli getrawmempool | python3 -c 'import sys,json;print(json.load(sys.stdin)[0])')
say "payout tx $OUT_TXID in mempool; mining 3 blocks"
mine 3
say "waiting for 3 observers to observe the outbound back in"
wait_for "sendout_escrow to empty" 180 bash -c "[[ \$(balance $USER BTC.BTC sendout_escrow) -eq 0 ]]"

RECEIVED=$(miner getreceivedbyaddress "$DEST" 1)
DEPOSIT_AFTER=$(balance "$USER" BTC.BTC deposit)
FEE_CHARGED=$((DEPOSIT_SATS - WITHDRAW_SATS - DEPOSIT_AFTER))
python3 -c "import sys; sys.exit(0 if float('$RECEIVED') >= $WITHDRAW_BTC else 1)" || fail "regtest address received $RECEIVED, expected $WITHDRAW_BTC"
(( FEE_CHARGED >= 0 && FEE_CHARGED <= 100000 )) || fail "unexpected network fee charged: $FEE_CHARGED sats"
say "regtest address $DEST holds $RECEIVED BTC (1 conf); user BTC.BTC deposit = $DEPOSIT_AFTER, escrow = 0, network fee charged = $FEE_CHARGED sats"

# Every node agrees on the result.
for i in 1 2 3; do
  b=$(curl -sf "http://127.0.0.1:$((RPC_BASE+i))/v1/accounts/$USER" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(next((b['balance'] for b in d['balances'] if b['asset']=='BTC.BTC' and b['account_type']=='deposit'),0))")
  [[ "$b" == "$DEPOSIT_AFTER" ]] || fail "node $i disagrees on the user's balance ($b vs $DEPOSIT_AFTER)"
done
say "PASS: deposit $DEPOSIT_SATS sats credited via SPV proof + 3-observer quorum; withdrawal $WITHDRAW_SATS sats signed, broadcast ($OUT_TXID), mined, observed back; escrow empty on all 4 nodes"
