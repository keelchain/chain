#!/usr/bin/env bash
# Custody fresh-pool rotation end-to-end (docs/models.md, "Rotating to a
# fresh pool") on a laptop: the client-vault setup, then
#
#   1. the client starts a second signer and rotates its Bitcoin vault to
#      that key (no sweep): the user's address changes, the old one is a
#      draining generation
#   2. a deposit to the OLD address still credits the user; one to the new
#      address credits too
#   3. a withdrawal is paid from the draining generation only: every input
#      is an old-generation coin and the change returns to the old key
#   4. the client rotates again with sweep to a third signer: the observers
#      sweep the previous generation to the new key, attested on chain
#   5. after the grace the observers report the swept generation empty and
#      the chain retires it; reserve == liabilities throughout, the network
#      reserve never moves, and every node agrees
#
# Env: KEEL_DEVNET_DIR, RPC_BASE, CARGO_TARGET_DIR, NO_BUILD=1,
#      BTC_RPC_PORT (default 18743), BTC_P2P_PORT (18744), KEEP=1.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-rotate}
export RPC_BASE=${RPC_BASE:-5600}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/chain/target}
export PATH="$HOME/.local/bin:$PATH"
KEEL="$CARGO_TARGET_DIR/release/keel"
OBS="$CARGO_TARGET_DIR/release/keel-observer"
RPC0="http://127.0.0.1:$RPC_BASE"
BTC_DIR="$KEEL_DEVNET_DIR/bitcoind"
BTC_RPC_PORT=${BTC_RPC_PORT:-18743}
BTC_P2P_PORT=${BTC_P2P_PORT:-18744}
BTC_AUTH=(-rpcuser=keel -rpcpassword=keel)
DEPOSIT_BTC=0.5;   DEPOSIT_SATS=50000000
WITHDRAW_BTC=0.2;  WITHDRAW_SATS=20000000
DEV_SEED=$(printf '%02x' $(seq 1 32) | tr -d '\n')   # 32-byte seed of the shared dev signer
CLIENT_SEED=$(printf '%02x' $(seq 101 132) | tr -d '\n')   # the client's own vault key
SIGNER_PORT=${SIGNER_PORT:-7400}
SIGNER_PORT2=$((SIGNER_PORT+1)); SIGNER_PORT3=$((SIGNER_PORT+2))
CLIENT_SEED2=$(printf '%02x' $(seq 133 164) | tr -d '\n')   # the fresh key of the first rotation
CLIENT_SEED3=$(printf '%02x' $(seq 165 196) | tr -d '\n')   # the fresh key of the second (swept) rotation
GAS_BTC=0.01;      GAS_SATS=1000000

say() { echo "[rotate-e2e] $*"; }
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
moves_secs = 3

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
# ---------------------------------------------------------------- helpers for generations
gens() { curl -sf "$RPC0/v1/custody/$CLIENT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(' '.join(f\"{v['gen']}:{v['status']}\" for v in d['vaults'] if v['chain']=='BTC'))"; }
gen_field() { # gen field
  curl -sf "$RPC0/v1/custody/$CLIENT" | python3 -c "
import sys,json
d=json.load(sys.stdin)
v=next(x for x in d['vaults'] if x['chain']=='BTC' and x['gen']==$1)
print(v['$2'])"; }
user_addr() { # gen
  curl -sf "$RPC0/v1/custody/$CLIENT/BTC/addresses?gen=$1" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(next(a['address'] for a in d['addresses'] if a['index']==$INDEX and a['owner']=='$USER'))"; }
lookup_gen() { curl -sf "$RPC0/v1/custody/$CLIENT/BTC/addresses/lookup?address=$1" | json 'd["gen"]'; }
start_signer() { # seed port log
  nohup "$OBS" dev-signer --seed "$1" --listen "127.0.0.1:$2" > "$3" 2>&1 &
  wait_for "signer on :$2" 30 bash -c "grep -q '\"public_key\"' $3"
}
rotate() { # seed port sweepflag
  local log="$KEEL_DEVNET_DIR/client-signer-$2.log"
  start_signer "$1" "$2" "$log"
  local pk cc
  pk=$(head -1 "$log" | json 'd["public_key"]'); cc=$(head -1 "$log" | json 'd["chain_code"]')
  "$KEEL" --rpc "$RPC0" send --secret "$CLIENT_SECRET" --wait 30 custody rotate BTC \
    --public-key "$pk" --chain-code "$cc" --signer-url "http://127.0.0.1:$2" $3 > "$KEEL_DEVNET_DIR/rotate-$2.json"
  grep -q '"PoolRotated"' "$KEEL_DEVNET_DIR/rotate-$2.json" || { cat "$KEEL_DEVNET_DIR/rotate-$2.json"; fail "rotation refused"; }
}
export -f gens gen_field user_addr lookup_gen json
export RPC0 CLIENT USER INDEX
# ---------------------------------------------------------------- 1. rotate (no sweep)
say "the client rotates its Bitcoin vault to a fresh key on a second signer (:$SIGNER_PORT2), no sweep"
rotate "$CLIENT_SEED2" "$SIGNER_PORT2" ""
wait_for "two live generations" 30 bash -c "[[ \"\$(gens)\" == '1:Active 0:Draining' ]]"
ADDR1=$(user_addr 1)
[[ "$ADDR1" != "$ADDR" ]] || fail "the user's address did not change with the key"
[[ "$(user_addr 0)" == "$ADDR" ]] || fail "the draining generation lost the user's old address"
[[ "$(lookup_gen "$ADDR")" == 0 && "$(lookup_gen "$ADDR1")" == 1 ]] || fail "reverse lookup across generations"
GAS_ADDR1=$(gen_field 1 address)
say "generations: $(gens); user index $INDEX -> old $ADDR (gen 0), new $ADDR1 (gen 1)"
# the observers rebuild their books every 3 s and import the new key's addresses
sleep 8
# ---------------------------------------------------------------- 2. deposits to both generations
say "depositing 0.1 BTC to the OLD address and 0.1 BTC to the NEW one; mining 3 blocks"
miner sendtoaddress "$ADDR" 0.1 >/dev/null
miner sendtoaddress "$ADDR1" 0.1 >/dev/null
mine 3
wait_for "both deposits credited to the same account" 180 bash -c "[[ \$(balance $USER BTC.BTC custody) -eq $((DEPOSIT_SATS + 20000000)) ]]"
USER_AFTER=$((DEPOSIT_SATS + 20000000))
read -r RES LIA HALT <<<"$(reserves)"
[[ "$RES" == "$LIA" && "$HALT" == False ]] || fail "reserve $RES vs liabilities $LIA halted=$HALT after the deposits"
[[ "$(balance "$SYSTEM" BTC.BTC vault_asset)" == "$NET_RESERVE_BEFORE" ]] || fail "the network reserve moved"
# ---------------------------------------------------------------- 3. a withdrawal drains the old generation
DEST=$(miner getnewaddress)
say "user withdraws $WITHDRAW_BTC BTC: it must be paid from the draining generation (gen 0) only"
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 custody withdraw BTC.BTC "$DEST" "$WITHDRAW_SATS" > "$KEEL_DEVNET_DIR/withdraw.json"
grep -q '"CustodyWithdrawalQueued"' "$KEEL_DEVNET_DIR/withdraw.json" || { cat "$KEEL_DEVNET_DIR/withdraw.json"; fail "custody withdrawal not queued"; }
wait_for "a batch pointed at generation 0" 240 bash -c "curl -sf $RPC0/v1/vaults/outbounds | python3 -c \"import sys,json; d=json.load(sys.stdin); sys.exit(0 if any(b.get('custodian')=='$CLIENT' and b.get('gen')==0 for b in d['batches']) else 1)\""
wait_for "payout in the regtest mempool" 180 bash -c "python3 -c \"import sys; sys.exit(0 if float('\$(miner getreceivedbyaddress $DEST 0)') >= $WITHDRAW_BTC else 1)\""
OUT_TXID=$(bcli getrawmempool | python3 -c 'import sys,json;print(json.load(sys.stdin)[0])')
bcli getrawtransaction "$OUT_TXID" 1 > "$KEEL_DEVNET_DIR/payout.json"
python3 -c "
import json,subprocess,sys
tx=json.load(open('$KEEL_DEVNET_DIR/payout.json'))
out={}
for vin in tx['vin']:
    out[vin['txid']]=json.loads(subprocess.check_output(['bitcoin-cli','-regtest','-datadir=$BTC_DIR','-rpcport=$BTC_RPC_PORT','-rpcuser=keel','-rpcpassword=keel','getrawtransaction',vin['txid'],'1']))
json.dump(out,open('$KEEL_DEVNET_DIR/prevouts.json','w'))"
# Resolve each input's address through its parent transaction (txindex is on;
# a mempool transaction carries no prevout itself).
python3 - "$KEEL_DEVNET_DIR/payout.json" "$RPC0" "$CLIENT" "$GAS_ADDR" "$GAS_ADDR1" "$KEEL_DEVNET_DIR/prevouts.json" <<'PYIN'
import json, sys, urllib.request, urllib.parse
tx=json.load(open(sys.argv[1])); rpc, client, gas0, gas1 = sys.argv[2:6]
prevouts=json.load(open(sys.argv[6]))
def gen_of(addr):
    with urllib.request.urlopen(f"{rpc}/v1/custody/{client}/BTC/addresses/lookup?address={urllib.parse.quote(addr)}") as r:
        return json.load(r)["gen"]
for vin in tx["vin"]:
    parent=prevouts[vin["txid"]]
    a=parent["vout"][vin["vout"]]["scriptPubKey"]["address"]
    g=gen_of(a)
    assert g==0, f"input {a} belongs to generation {g}, not the draining one"
outs=[o["scriptPubKey"].get("address") for o in tx["vout"]]
assert gas1 not in outs, "change went to the new generation: the pools got linked"
print("payout inputs all gen 0; outputs:", outs)
PYIN
mine 3
wait_for "custody escrow to empty" 180 bash -c "[[ \$(balance $USER BTC.BTC custody_escrow) -eq 0 ]]"
USER_AFTER=$((USER_AFTER - WITHDRAW_SATS))
[[ "$(balance "$USER" BTC.BTC custody)" == "$USER_AFTER" ]] || fail "user custody balance after the withdrawal"
read -r RES LIA HALT <<<"$(reserves)"
[[ "$RES" == "$LIA" && "$HALT" == False ]] || fail "reserve $RES vs liabilities $LIA halted=$HALT after the withdrawal"
# ---------------------------------------------------------------- 4. rotate again, with sweep
say "the client (param admin on the devnet) shortens the grace to 5 blocks, then rotates with --sweep to a third signer (:$SIGNER_PORT3)"
"$KEEL" --rpc "$RPC0" send --secret "$CLIENT_SECRET" --wait 30 raw '{"SetParam":{"key":"pools.grace_blocks","value":5}}' > "$KEEL_DEVNET_DIR/grace.json"
grep -qE '"ok": *true' "$KEEL_DEVNET_DIR/grace.json" || { cat "$KEEL_DEVNET_DIR/grace.json"; fail "could not set pools.grace_blocks"; }
rotate "$CLIENT_SEED3" "$SIGNER_PORT3" "--sweep"
wait_for "three generations, 1 draining under sweep" 30 bash -c "[[ \"\$(gens)\" == '2:Active 1:Draining 0:Draining' && \"\$(gen_field 1 sweep)\" == True ]]"
GAS_ADDR2=$(gen_field 2 address)
sleep 8
say "waiting for the observers to sweep generation 1 (0.1 BTC) to the new key's own address $GAS_ADDR2"
wait_for "sweep in the mempool" 240 bash -c "[[ \$(bcli getrawmempool | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))') -ge 1 ]]"
mine 3
wait_for "sweep attested on chain" 240 bash -c "[[ \"\$(gen_field 1 swept)\" != '[]' ]]"
SWEPT=$(gen_field 1 swept)
say "generation 1 swept: $SWEPT"
RECV2=$(bcli -rpcwallet=keel-observer-0 getreceivedbyaddress "$GAS_ADDR2" 1 2>/dev/null || echo 0)
python3 -c "import sys; sys.exit(0 if float('$RECV2') >= 0.099 else 1)" || fail "the new key's own address received $RECV2 BTC, expected the swept 0.1 minus fee"
[[ "$(balance "$USER" BTC.BTC custody)" == "$USER_AFTER" ]] || fail "a sweep changed a user balance"
read -r RES LIA HALT <<<"$(reserves)"
[[ "$RES" == "$LIA" && "$HALT" == False ]] || fail "reserve $RES vs liabilities $LIA halted=$HALT after the sweep"
# ---------------------------------------------------------------- 5. retirement after the grace
say "mining past the grace; the observers report generation 1 empty and the chain retires it"
mine 6
wait_for "generation 1 retired" 240 bash -c "[[ \"\$(gens)\" == '2:Active 0:Draining' ]]"
[[ "$(balance "$SYSTEM" BTC.BTC vault_asset)" == "$NET_RESERVE_BEFORE" ]] || fail "the network reserve moved"
for i in 1 2 3; do
  b=$(curl -sf "http://127.0.0.1:$((RPC_BASE+i))/v1/accounts/$USER" | python3 -c "
import sys,json
d=json.load(sys.stdin)
print(next((b['balance'] for b in d['balances'] if b['asset']=='BTC.BTC' and b['account_type']=='custody'),0))")
  [[ "$b" == "$USER_AFTER" ]] || fail "node $i disagrees on the user's custody balance ($b vs $USER_AFTER)"
done
say "PASS: rotated twice ($(gens)); old and new addresses both credited; withdrawal paid from the draining key only; generation 1 swept to the fresh key ($SWEPT) and retired after the grace; user custody $USER_AFTER sats; reserve == liabilities == $RES; network reserve untouched"
