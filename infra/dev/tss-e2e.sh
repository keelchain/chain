#!/usr/bin/env bash
# Threshold custody end-to-end on one machine: the 4-validator devnet, a
# regtest bitcoind, THREE `keel-tss serve` daemons holding 2-of-3 shares of a
# vault key created by a real distributed key generation, and three observer
# daemons that sign through them. Every signer enforces the signing policy
# against node RPC, so a withdrawal is signed only because it pays an open
# outbound of a finalized batch; a request with a bogus context is refused.
#
# Steps: devnet + regtest (as custody-e2e.sh); keygen ceremony (primes are
# cached under ~/.cache/keel-tss); vault registered with the threshold key;
# observers with tss_url = the local signer; deposit credited by SPV proof
# and quorum; withdrawal signed by two signers, broadcast, mined, observed
# back; then a forged /sign request is refused with 403.
#
# Env: KEEL_DEVNET_DIR, RPC_BASE, CARGO_TARGET_DIR, NO_BUILD=1, KEEP=1,
#      BTC_RPC_PORT (18643), BTC_P2P_PORT (18644), TSS_BASE (7300),
#      DEVNET_IDLE_MS (1000).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export KEEL_DEVNET_DIR=${KEEL_DEVNET_DIR:-/tmp/keel-tss-e2e}
export RPC_BASE=${RPC_BASE:-5300}
export DEVNET_IDLE_MS=${DEVNET_IDLE_MS:-1000}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/chain/target}
export PATH="$HOME/.local/bin:$PATH"
KEEL="$CARGO_TARGET_DIR/release/keel"
OBS="$CARGO_TARGET_DIR/release/keel-observer"
TSS="$CARGO_TARGET_DIR/release/keel-tss"
RPC0="http://127.0.0.1:$RPC_BASE"
BTC_DIR="$KEEL_DEVNET_DIR/bitcoind"
BTC_RPC_PORT=${BTC_RPC_PORT:-18643}
BTC_P2P_PORT=${BTC_P2P_PORT:-18644}
TSS_BASE=${TSS_BASE:-7300}
TSS_DIR="$KEEL_DEVNET_DIR/tss"
PRIMES_CACHE="$HOME/.cache/keel-tss"
BTC_AUTH=(-rpcuser=keel -rpcpassword=keel)
DEPOSIT_BTC=0.5;   DEPOSIT_SATS=50000000
WITHDRAW_BTC=0.2;  WITHDRAW_SATS=20000000
export KEEL_TSS_PASSPHRASE=e2e-passphrase
export KEEL_TSS_SECRET=$(printf '%02x' $(seq 101 132) | tr -d '\n')

say() { echo "[tss-e2e] $*"; }
fail() { say "FAIL: $*"; exit 1; }
json() { python3 -c "import sys,json
lines=[l for l in sys.stdin.read().splitlines() if l.strip().startswith(('{','['))]
d=json.loads(lines[-1]) if lines else None
print($1)"; }
height() { curl -sf "$1/v1/status" | json 'd["height"]' 2>/dev/null || echo 0; }
balance() { curl -sf "$RPC0/v1/accounts/$1" | python3 -c "
import sys,json
d=json.load(sys.stdin)
for b in d['balances']:
    if b['asset']=='$2' and b['account_type']=='$3': print(b['balance']); break
else: print(0)"; }
wait_for() { local what=$1 t=$2; shift 2; while (( t > 0 )); do "$@" && return 0; sleep 1; t=$((t-1)); done; fail "timeout waiting for $what"; }
bcli() { bitcoin-cli -regtest -datadir="$BTC_DIR" -rpcport="$BTC_RPC_PORT" -rpcuser=keel -rpcpassword=keel "$@"; }
miner() { bcli -rpcwallet=miner "$@"; }
mine() { miner generatetoaddress "$1" "$(miner getnewaddress)" >/dev/null; }
export BTC_DIR BTC_RPC_PORT RPC0
export -f json height balance bcli miner

cleanup() {
  [[ "${KEEP:-}" == 1 ]] && { say "KEEP=1: leaving everything running"; return; }
  pkill -f "keel-observer run --config $KEEL_DEVNET_DIR" >/dev/null 2>&1 || true
  pkill -f "keel-tss serve --share $TSS_DIR" >/dev/null 2>&1 || true
  bcli stop >/dev/null 2>&1 || true
  "$ROOT/infra/dev/devnet.sh" stop >/dev/null 2>&1 || true
}
trap cleanup EXIT

find_bitcoind() {
  command -v bitcoind >/dev/null 2>&1 && return 0
  for d in "$HOME/.local/bin" /usr/local/bin /opt/bitcoin/bin; do [[ -x "$d/bitcoind" ]] && { export PATH="$d:$PATH"; return 0; }; done
  say "bitcoind not on PATH; downloading Bitcoin Core into ~/.local/bin"
  local arch ver=29.1 tmp
  case "$(uname -m)" in x86_64) arch=x86_64-linux-gnu;; aarch64|arm64) arch=aarch64-linux-gnu;; *) return 1;; esac
  tmp=$(mktemp -d); mkdir -p "$HOME/.local/bin"
  if curl -sfL --max-time 240 -o "$tmp/btc.tar.gz" "https://bitcoincore.org/bin/bitcoin-core-$ver/bitcoin-$ver-$arch.tar.gz" \
     && tar xzf "$tmp/btc.tar.gz" -C "$tmp" && cp "$tmp/bitcoin-$ver/bin/bitcoind" "$tmp/bitcoin-$ver/bin/bitcoin-cli" "$HOME/.local/bin/"; then
    rm -rf "$tmp"; export PATH="$HOME/.local/bin:$PATH"; return 0; fi
  rm -rf "$tmp"; return 1
}
find_bitcoind || { say "SKIP: no bitcoind available"; trap - EXIT; exit 0; }

[[ "${NO_BUILD:-}" == 1 ]] || (cd "$ROOT/chain" && cargo build --release -p keel-node -p keel-cli -p keel-observer -p keel-tss)
[[ -x "$KEEL" && -x "$OBS" && -x "$TSS" ]] || fail "missing binaries under $CARGO_TARGET_DIR/release"
# devnet.sh start recreates $KEEL_DEVNET_DIR: make the subdirectories after it.
NO_BUILD=1 "$ROOT/infra/dev/devnet.sh" start
mkdir -p "$KEEL_DEVNET_DIR/observers" "$BTC_DIR" "$TSS_DIR" "$PRIMES_CACHE"
at_height() { [[ "$(height "$1")" =~ ^[0-9]+$ ]] && (( $(height "$1") >= $2 )); }
for i in 0 1 2 3; do wait_for "height 3 on node $i" 90 at_height "http://127.0.0.1:$((RPC_BASE+i))" 3; done

# ---------------------------------------------------------------- ceremony
PEERS="127.0.0.1:$TSS_BASE,127.0.0.1:$((TSS_BASE+1)),127.0.0.1:$((TSS_BASE+2))"
for i in 0 1 2; do
  if [[ ! -f "$PRIMES_CACHE/primes-$i.json" ]]; then
    say "generating Paillier primes for party $i (once; cached in $PRIMES_CACHE)"
    "$TSS" gen-primes --out "$PRIMES_CACHE/primes-$i.json" >/dev/null
  fi
done
say "distributed key generation, 2-of-3"
for i in 0 1 2; do
  "$TSS" keygen --index "$i" --n 3 --t 2 --peers "$PEERS" --eid "keel-vault:e2e:epoch:1" \
    --primes "$PRIMES_CACHE/primes-$i.json" --out "$TSS_DIR/share-$i.enc" --timeout-secs 300 > "$TSS_DIR/keygen-$i.json" 2> "$TSS_DIR/keygen-$i.log" &
done
wait
for i in 0 1 2; do [[ -s "$TSS_DIR/keygen-$i.json" ]] || { cat "$TSS_DIR/keygen-$i.log"; fail "keygen $i produced no key"; }; done
# keygen prints pretty JSON (several lines): read the whole file.
jfile() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2"; }
PUB=$(jfile "$TSS_DIR/keygen-0.json" public_key); CC=$(jfile "$TSS_DIR/keygen-0.json" chain_code)
for i in 1 2; do [[ "$(jfile "$TSS_DIR/keygen-$i.json" public_key)" == "$PUB" ]] || fail "party $i derived a different key"; done
say "vault key $PUB"

# ---------------------------------------------------------------- signers (with policy)
for i in 0 1 2; do
  RUST_LOG=${RUST_LOG:-info} nohup "$TSS" serve --share "$TSS_DIR/share-$i.enc" --peers "$PEERS" \
    --http "127.0.0.1:$((TSS_BASE+100+i))" --policy-rpc "http://127.0.0.1:$((RPC_BASE+i))" > "$TSS_DIR/serve-$i.log" 2>&1 &
  say "signer $i pid $! http $((TSS_BASE+100+i)) policy rpc $((RPC_BASE+i))"
done
sleep 2

# ---------------------------------------------------------------- regtest
nohup bitcoind -regtest -datadir="$BTC_DIR" -rpcport="$BTC_RPC_PORT" -port="$BTC_P2P_PORT" "${BTC_AUTH[@]}" \
  -listen=0 -fallbackfee=0.0001 -txindex=1 > "$KEEL_DEVNET_DIR/bitcoind.log" 2>&1 &
wait_for "bitcoind rpc" 60 bash -c "bitcoin-cli -regtest -datadir='$BTC_DIR' -rpcport=$BTC_RPC_PORT ${BTC_AUTH[*]} getblockchaininfo >/dev/null 2>&1"
bcli createwallet miner >/dev/null
mine 101

# ---------------------------------------------------------------- keys, observers
keyof() { "$KEEL" keygen --seed "$1" | json "d['$2']"; }
declare -a OBS_ADDR OBS_SECRET
for i in 0 1 2; do OBS_ADDR[i]=$(keyof "$i" address); OBS_SECRET[i]=$(keyof "$i" secret); done
USER_SECRET=$(keyof 42 secret); USER=$(keyof 42 address)
SIGNERS="${OBS_ADDR[0]},${OBS_ADDR[1]},${OBS_ADDR[2]}"
for i in 0 1 2; do
cat > "$KEEL_DEVNET_DIR/observers/$i.toml" <<EOF
keel_rpc_url = "http://127.0.0.1:$((RPC_BASE+i))"
tss_url = "http://127.0.0.1:$((TSS_BASE+100+i))"
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

say "observer 0 registers the Bitcoin vault with the threshold key (signers = observers 0..2, threshold 2)"
REG=$(KEEL_OBSERVER_SECRET=${OBS_SECRET[0]} "$OBS" register-vault --config "$KEEL_DEVNET_DIR/observers/0.toml" \
      --chain BTC --epoch 1 --public-key "$PUB" --chain-code "$CC" --signers "$SIGNERS" --threshold 2)
echo "$REG" | grep -q '"admitted":true' || fail "register-vault refused: $REG"
TX=$(echo "$REG" | json 'd["tx_id"]')
wait_for "vault registration receipt" 30 bash -c "curl -sf $RPC0/v1/receipts/$TX | grep -q '\"ok\":true'"

for i in 0 1 2; do
  KEEL_OBSERVER_SECRET=${OBS_SECRET[i]} RUST_LOG=${RUST_LOG:-info} nohup "$OBS" run --config "$KEEL_DEVNET_DIR/observers/$i.toml" \
    > "$KEEL_DEVNET_DIR/observers/$i.log" 2>&1 &
done
wait_for "observer wallets" 60 bash -c "[[ \$(bcli listwallets | grep -c keel-observer) -eq 3 ]]"

# ---------------------------------------------------------------- deposit
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 raw '{"RequestDepositAddress":{"chain":"Bitcoin"}}' > "$KEEL_DEVNET_DIR/deposit-address.json"
INDEX=$(python3 -c "
import json
d=json.load(open('$KEEL_DEVNET_DIR/deposit-address.json'))
print([e['DepositAddressAssigned']['index'] for e in d['events'] if 'DepositAddressAssigned' in e][0])")
ADDR=$(KEEL_OBSERVER_SECRET=${OBS_SECRET[0]} "$OBS" addresses --config "$KEEL_DEVNET_DIR/observers/0.toml" --chain BTC | awk -v i="$INDEX" -v u="$USER" '$1==i && $3==u {print $2}')
[[ -n "$ADDR" ]] || fail "no deposit address for index $INDEX"
say "deposit index $INDEX -> $ADDR"
DEP_TXID=$(miner sendtoaddress "$ADDR" "$DEPOSIT_BTC"); mine 3
wait_for "BTC.BTC credit on chain" 180 bash -c "[[ \$(balance $USER BTC.BTC deposit) -eq $DEPOSIT_SATS ]]"
say "credited $DEPOSIT_SATS sats (tx $DEP_TXID)"

# ---------------------------------------------------------------- withdrawal through the threshold signers
DEST=$(miner getnewaddress)
"$KEEL" --rpc "$RPC0" send --secret "$USER_SECRET" --wait 30 withdraw BTC.BTC "$DEST" "$WITHDRAW_SATS" > "$KEEL_DEVNET_DIR/withdraw.json"
grep -q '"WithdrawalQueued"' "$KEEL_DEVNET_DIR/withdraw.json" || fail "withdrawal not queued"
say "waiting for the batch to be threshold-signed and broadcast"
wait_for "payout in the regtest mempool" 300 bash -c "python3 -c \"import sys; sys.exit(0 if float('\$(miner getreceivedbyaddress $DEST 0)') >= $WITHDRAW_BTC else 1)\""
OUT_TXID=$(bcli getrawmempool | python3 -c 'import sys,json;print(json.load(sys.stdin)[0])')
mine 3
wait_for "sendout_escrow to empty" 180 bash -c "[[ \$(balance $USER BTC.BTC sendout_escrow) -eq 0 ]]"
RECEIVED=$(miner getreceivedbyaddress "$DEST" 1)
python3 -c "import sys; sys.exit(0 if float('$RECEIVED') >= $WITHDRAW_BTC else 1)" || fail "regtest address received $RECEIVED"
grep -q "joined signing session" "$TSS_DIR"/serve-*.log || fail "no signer joined a session"
! grep -q "refused by policy" "$TSS_DIR"/serve-*.log || fail "the policy refused a legitimate request"
say "withdrawal $OUT_TXID signed by two of three shares, mined, observed back"

# ---------------------------------------------------------------- the policy refuses a forged request
say "a signing request without a chain context is refused"
CODE=$(curl -s -o "$TSS_DIR/forged.json" -w '%{http_code}' -X POST "http://127.0.0.1:$((TSS_BASE+100))/sign" \
  -H 'content-type: application/json' -d "{\"digest\":\"$(printf 'ab%.0s' $(seq 1 32))\",\"path\":[0,0]}")
[[ "$CODE" == 403 ]] || { cat "$TSS_DIR/forged.json"; fail "forged request answered $CODE, expected 403"; }
say "a request whose context names a batch that pays nobody is refused"
BOGUS_TX=$(bcli createrawtransaction "[]" "{\"$DEST\":0.01}")
CODE=$(curl -s -o "$TSS_DIR/forged2.json" -w '%{http_code}' -X POST "http://127.0.0.1:$((TSS_BASE+100))/sign" \
  -H 'content-type: application/json' -d "{\"digest\":\"$(printf 'ab%.0s' $(seq 1 32))\",\"path\":[0,0],\"context\":{\"chain\":\"BTC\",\"batch_id\":999,\"raw_tx\":\"$BOGUS_TX\",\"input\":0,\"prevout_value\":1000,\"prevout_pubkey\":\"$PUB\",\"network\":\"regtest\"}}")
[[ "$CODE" == 403 ]] || { cat "$TSS_DIR/forged2.json"; fail "forged batch answered $CODE, expected 403"; }
grep -q "refused by policy" "$TSS_DIR/serve-0.log" || fail "signer 0 did not log the refusal"
say "PASS: 2-of-3 threshold custody with policy: deposit credited, withdrawal signed and paid, forged requests refused"
