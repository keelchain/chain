#!/usr/bin/env bash
# Client-owned vault end-to-end (docs/models.md, Model A) on a laptop: the
# 4-validator devnet, a regtest bitcoind, three observers on the shared
# development signer for the NETWORK vault, and a separate development
# signer over HTTP standing in for a client's `keel-tss serve`.
#
#   1. devnet + regtest + network vault + observers, as custody-e2e.sh
#   2. the client (genesis attester 0) starts its own signer and registers
#      a custody vault on Bitcoin with that signer's key and URL
#   3. the client attests a user; the user asks for an address in the
#      client's vault; regtest coins sent there are credited as a CUSTODY
#      balance backed by the client's reserve (the network reserve is
#      untouched)
#   4. the client tops up gas at its vault's own address (index 0)
#   5. the user withdraws through the client's vault: the observers build
#      the batch, the client's signer signs it, it is broadcast, mined and
#      observed back; the fee came out of the client's gas; reserve equals
#      liabilities and nothing is halted
#
# Env: KEEL_DEVNET_DIR, RPC_BASE, CARGO_TARGET_DIR, NO_BUILD=1,
#      BTC_RPC_PORT (default 18543), BTC_P2P_PORT (18544), KEEP=1 (do not
#      stop anything at exit).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-client-vault}
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
CLIENT_SEED=$(printf '%02x' $(seq 101 132) | tr -d '\n')   # the client's own vault key
SIGNER_PORT=${SIGNER_PORT:-7300}
GAS_BTC=0.01;      GAS_SATS=1000000

say() { echo "[client-vault-e2e] $*"; }
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
  pkill -f "keel-observer dev-signer --seed $CLIENT_SEED" >/dev/null 2>&1 || true
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

# ---------------------------------------------------------------- the client's own signer and vault
CLIENT=${OBS_ADDR[0]}; CLIENT_SECRET=${OBS_SECRET[0]}   # genesis attester 0 plays the client
say "client ${CLIENT:0:8}… starts its own signer on :$SIGNER_PORT"
nohup "$OBS" dev-signer --seed "$CLIENT_SEED" --listen "127.0.0.1:$SIGNER_PORT" > "$KEEL_DEVNET_DIR/client-signer.log" 2>&1 &
wait_for "client signer" 30 bash -c "grep -q '\"public_key\"' $KEEL_DEVNET_DIR/client-signer.log"
CPK=$(head -1 "$KEEL_DEVNET_DIR/client-signer.log" | json 'd["public_key"]')
CCC=$(head -1 "$KEEL_DEVNET_DIR/client-signer.log" | json 'd["chain_code"]')
[[ ${#CPK} -eq 66 && ${#CCC} -eq 64 ]] || fail "client signer printed no usable key"
say "client registers its Bitcoin custody vault (key ${CPK:0:12}…, signer http://127.0.0.1:$SIGNER_PORT)"
"$KEEL" --rpc "$RPC0" send --secret "$CLIENT_SECRET" --wait 30 custody register BTC --epoch 1 \
  --public-key "$CPK" --chain-code "$CCC" --signer-url "http://127.0.0.1:$SIGNER_PORT" > "$KEEL_DEVNET_DIR/register.json"
grep -q '"CustodyVaultRegistered"' "$KEEL_DEVNET_DIR/register.json" || { cat "$KEEL_DEVNET_DIR/register.json"; fail "custody vault not registered"; }
wait_for "custody vault on the node" 30 bash -c "curl -sf $RPC0/v1/custody/$CLIENT | grep -q '\"signer_url\"'"
GAS_ADDR=$(curl -sf "$RPC0/v1/custody/$CLIENT" | json 'd["vaults"][0]["address"]')
[[ -n "$GAS_ADDR" && "$GAS_ADDR" != None ]] || fail "no index-0 address for the client vault"
# ---------------------------------------------------------------- the client's user
EXP=$(( $(date +%s) + 30*86400 ))
say "client attests the user; the user asks for an address in the client's vault"
"$KEEL" --rpc "$RPC0" send --secret "$CLIENT_SECRET" --wait 30 raw "{\"Attest\":{\"subject\":\"$USER\",\"tier\":1,\"expires_at\":$EXP}}" > "$KEEL_DEVNET_DIR/attest.json"
grep -q '"Attested"' "$KEEL_DEVNET_DIR/attest.json" || { cat "$KEEL_DEVNET_DIR/attest.json"; fail "attest failed"; }
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 custody address BTC "$CLIENT" > "$KEEL_DEVNET_DIR/custody-address.json"
INDEX=$(python3 -c "
import json
d=json.load(open('$KEEL_DEVNET_DIR/custody-address.json'))
print([e['CustodyAddressAssigned']['index'] for e in d['events'] if 'CustodyAddressAssigned' in e][0])")
ADDR=$(curl -sf "$RPC0/v1/custody/$CLIENT/BTC/addresses" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(next(a['address'] for a in d['addresses'] if a['index']==$INDEX and a['owner']=='$USER'))")
[[ -n "$ADDR" ]] || fail "no custody address for index $INDEX"
say "custody index $INDEX -> $ADDR (gas address $GAS_ADDR)"
SYSTEM=$(printf '0%.0s' $(seq 1 64))
NET_RESERVE_BEFORE=$(balance "$SYSTEM" BTC.BTC vault_asset)
# ---------------------------------------------------------------- deposit + gas
say "sending $DEPOSIT_BTC BTC to the user's custody address and $GAS_BTC BTC of gas to the vault's own address; mining 3 blocks"
DEP_TXID=$(miner sendtoaddress "$ADDR" "$DEPOSIT_BTC")
GAS_TXID=$(miner sendtoaddress "$GAS_ADDR" "$GAS_BTC")
mine 3
say "deposits $DEP_TXID / $GAS_TXID mined; waiting for the observers' attestations"
wait_for "custody credit" 180 bash -c "[[ \$(balance $USER BTC.BTC custody) -eq $DEPOSIT_SATS ]]"
wait_for "gas credit" 180 bash -c "[[ \$(balance $CLIENT BTC.BTC custody) -eq $GAS_SATS ]]"
[[ "$(balance "$USER" BTC.BTC deposit)" == 0 ]] || fail "the deposit landed in a network-backed balance"
[[ "$(balance "$SYSTEM" BTC.BTC vault_asset)" == "$NET_RESERVE_BEFORE" ]] || fail "the network reserve moved"
reserves() { curl -sf "$RPC0/v1/custody/$CLIENT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
r=next(x for x in d['vaults'][0]['reserves'] if x['asset']=='BTC.BTC')
print(r['reserve'], r['liabilities'], r['halted'])"; }
read -r RES LIA HALT <<<"$(reserves)"
say "client reserve $RES sats vs liabilities $LIA (halted=$HALT)"
[[ "$RES" == "$LIA" && "$RES" == $((DEPOSIT_SATS + GAS_SATS)) && "$HALT" == False ]] || fail "reserve/liabilities mismatch after deposit"
# ---------------------------------------------------------------- withdrawal through the client's vault
DEST=$(miner getnewaddress)
say "user withdraws $WITHDRAW_BTC BTC through the client's vault to $DEST"
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 custody withdraw BTC.BTC "$DEST" "$WITHDRAW_SATS" > "$KEEL_DEVNET_DIR/withdraw.json"
grep -q '"CustodyWithdrawalQueued"' "$KEEL_DEVNET_DIR/withdraw.json" || { cat "$KEEL_DEVNET_DIR/withdraw.json"; fail "custody withdrawal not queued"; }
[[ "$(balance "$USER" BTC.BTC custody_escrow)" == "$WITHDRAW_SATS" ]] || fail "custody escrow is not $WITHDRAW_SATS"
wait_for "batch with the client as custodian" 60 bash -c "curl -sf $RPC0/v1/vaults/outbounds | python3 -c \"import sys,json; d=json.load(sys.stdin); sys.exit(0 if any(b.get('custodian')=='$CLIENT' for b in d['batches']) else 1)\""
say "waiting for an observer to build the batch, the client's signer to sign it, and the broadcast"
wait_for "payout in the regtest mempool" 180 bash -c "python3 -c \"import sys; sys.exit(0 if float('\$(miner getreceivedbyaddress $DEST 0)') >= $WITHDRAW_BTC else 1)\""
OUT_TXID=$(bcli getrawmempool | python3 -c 'import sys,json;print(json.load(sys.stdin)[0])')
grep -q "POST /sign\|sign" "$KEEL_DEVNET_DIR/client-signer.log" || say "note: signer log has no request line (fine, axum logs are quiet)"
say "payout tx $OUT_TXID in mempool; mining 3 blocks"
mine 3
wait_for "custody escrow to empty" 180 bash -c "[[ \$(balance $USER BTC.BTC custody_escrow) -eq 0 ]]"
RECEIVED=$(miner getreceivedbyaddress "$DEST" 1)
python3 -c "import sys; sys.exit(0 if float('$RECEIVED') >= $WITHDRAW_BTC else 1)" || fail "regtest address received $RECEIVED, expected $WITHDRAW_BTC"
USER_AFTER=$(balance "$USER" BTC.BTC custody)
GAS_AFTER=$(balance "$CLIENT" BTC.BTC custody)
FEE=$((GAS_SATS - GAS_AFTER))
[[ "$USER_AFTER" == $((DEPOSIT_SATS - WITHDRAW_SATS)) ]] || fail "user custody balance $USER_AFTER"
(( FEE > 0 && FEE <= 100000 )) || fail "unexpected network fee taken from the client's gas: $FEE sats"
read -r RES LIA HALT <<<"$(reserves)"
[[ "$RES" == "$LIA" && "$HALT" == False ]] || fail "reserve $RES vs liabilities $LIA halted=$HALT after the withdrawal"
[[ "$(balance "$SYSTEM" BTC.BTC vault_asset)" == "$NET_RESERVE_BEFORE" ]] || fail "the network reserve moved"
for i in 1 2 3; do
  b=$(curl -sf "http://127.0.0.1:$((RPC_BASE+i))/v1/accounts/$USER" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(next((b['balance'] for b in d['balances'] if b['asset']=='BTC.BTC' and b['account_type']=='custody'),0))")
  [[ "$b" == "$USER_AFTER" ]] || fail "node $i disagrees on the user's custody balance ($b vs $USER_AFTER)"
done
say "PASS: $DEPOSIT_SATS sats credited as custody in the client's vault; $WITHDRAW_SATS sats withdrawn through the client's own signer ($OUT_TXID); fee $FEE sats from the client's gas; reserve == liabilities == $RES; network reserve untouched"
