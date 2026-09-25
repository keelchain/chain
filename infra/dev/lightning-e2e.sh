#!/usr/bin/env bash
# Lightning end-to-end on the running custody stack (2026-09-10):
# two LND nodes on the regtest bitcoind (A = observer 0's node, B = the
# "user's wallet"), a channel between them, observer 0 restarted with a
# [lightning] section, the pool funded from the vault, then a deposit
# (B pays an invoice bound to a chain address) and a withdrawal (the chain
# assigns the payout to observer 0, LND A pays B's invoice).
#
# Prereqs: custody-e2e.sh KEEP=1 stack up (devnet :5000, bitcoind :18543,
# observers 0..2 with the dev signer), lnd/lncli on PATH (~/.local/bin).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
export PATH="$HOME/.local/bin:$PATH"
D=${KEEL_DEVNET_DIR:-/tmp/keel-custody}
KEEL="$ROOT/chain/target/release/keel"
OBS="$ROOT/chain/target/release/keel-observer"
RPC=${RPC:-http://127.0.0.1:5000}
BTC_RPC_PORT=${BTC_RPC_PORT:-18543}
say() { echo "[lightning-e2e] $*"; }
fail() { say "FAIL: $*"; exit 1; }
json() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)"; }
bcli() { bitcoin-cli -regtest -datadir="$D/bitcoind" -rpcport="$BTC_RPC_PORT" -rpcuser=keel -rpcpassword=keel "$@"; }
wait_for() { local what=$1 t=$2; shift 2; while (( t > 0 )); do "$@" >/dev/null 2>&1 && return 0; sleep 1; t=$((t-1)); done; fail "timeout waiting for $what"; }
keyof() { "$KEEL" keygen --seed "$1" | json "d['$2']"; }

command -v lnd >/dev/null || fail "lnd not on PATH"
curl -sf "$RPC/v1/status" >/dev/null || fail "no Keel node at $RPC"
bcli getblockchaininfo >/dev/null || fail "no regtest bitcoind"

# ---------------------------------------------------------------- LND nodes
lnd_start() { # name p2p grpc rest
  local name=$1 p2p=$2 grpc=$3 rest=$4 dir="$D/lnd-$1"
  if [[ -f "$dir/lnd.pid" ]] && kill -0 "$(cat "$dir/lnd.pid")" 2>/dev/null; then say "lnd $name already running"; return; fi
  mkdir -p "$dir"
  nohup lnd --lnddir="$dir" --bitcoin.regtest --bitcoin.node=bitcoind \
    --bitcoind.rpchost=127.0.0.1:$BTC_RPC_PORT --bitcoind.rpcuser=keel --bitcoind.rpcpass=keel \
    --bitcoind.rpcpolling --bitcoind.blockpollinginterval=2s --bitcoind.txpollinginterval=2s \
    --noseedbackup --listen=127.0.0.1:$p2p --rpclisten=127.0.0.1:$grpc --restlisten=127.0.0.1:$rest \
    --externalip=127.0.0.1:$p2p --protocol.wumbo-channels --accept-keysend --debuglevel=info \
    --bitcoin.defaultchanconfs=1 > "$dir/lnd.log" 2>&1 &
  echo $! > "$dir/lnd.pid"
  say "lnd $name pid $! rest :$rest log $dir/lnd.log"
}
lncli_() { local name=$1; shift; lncli --lnddir="$D/lnd-$name" --network=regtest --rpcserver=127.0.0.1:$([[ $name == a ]] && echo 10019 || echo 10020) "$@"; }
lnd_start a 9745 10019 8180
lnd_start b 9746 10020 8181
for n in a b; do
  port=$([[ $n == a ]] && echo 10019 || echo 10020)
  wait_for "lnd $n" 90 lncli_ "$n" getinfo
  wait_for "lnd $n active" 180 bash -c "lncli --lnddir='$D/lnd-$n' --network=regtest --rpcserver=127.0.0.1:$port state | grep -q SERVER_ACTIVE"
done
A_ID=$(lncli_ a getinfo | json "d['identity_pubkey']")
B_ID=$(lncli_ b getinfo | json "d['identity_pubkey']")
say "lnd A $A_ID  lnd B $B_ID"

# ---------------------------------------------------------------- on-chain funds + channel
if [[ "$(lncli_ a listchannels | json "len(d['channels'])")" == "0" ]]; then
  A_ADDR=$(lncli_ a newaddress p2wkh | json "d['address']")
  B_ADDR=$(lncli_ b newaddress p2wkh | json "d['address']")
  bcli -rpcwallet=miner sendtoaddress "$A_ADDR" 1 >/dev/null
  bcli -rpcwallet=miner sendtoaddress "$B_ADDR" 1 >/dev/null
  bcli generatetoaddress 6 "$(bcli -rpcwallet=miner getnewaddress)" >/dev/null
  wait_for "lnd A synced" 60 bash -c "lncli --lnddir='$D/lnd-a' --network=regtest --rpcserver=127.0.0.1:10019 getinfo | grep -q '\"synced_to_chain\": true'"
  wait_for "lnd A balance" 60 bash -c "[ \"\$(lncli --lnddir='$D/lnd-a' --network=regtest --rpcserver=127.0.0.1:10019 walletbalance | python3 -c 'import sys,json; print(int(json.load(sys.stdin)[\"confirmed_balance\"]))')\" -gt 0 ]"
  lncli_ a connect "$B_ID@127.0.0.1:9746" >/dev/null || true
  # A funds a 5M-sat channel and pushes 2M to B: both sides can pay.
  lncli_ a openchannel --node_key "$B_ID" --local_amt 5000000 --push_amt 2000000 >/dev/null
  bcli generatetoaddress 6 "$(bcli -rpcwallet=miner getnewaddress)" >/dev/null
  wait_for "channel active" 90 bash -c "lncli --lnddir='$D/lnd-a' --network=regtest --rpcserver=127.0.0.1:10019 listchannels | grep -q '\"active\": true'"
fi
say "channel: $(lncli_ a listchannels | json "[(c['local_balance'], c['remote_balance']) for c in d['channels']]")"

# ---------------------------------------------------------------- observer 0 with [lightning]
OBS0_SECRET=$(keyof 0 secret)
OBS0_ADDR=$(keyof 0 address)
CFG="$D/observers/0.toml"
if ! grep -q '^\[lightning\]' "$CFG"; then
  cat >> "$CFG" <<EOF

[lightning]
rest_url = "https://127.0.0.1:8180"
macaroon_path = "$D/lnd-a/data/chain/bitcoin/regtest/admin.macaroon"
tls_insecure = true
api_listen = "127.0.0.1:7201"
invoice_expiry_secs = 3600
poll_secs = 2
EOF
fi
OLD=$(pgrep -f "observers/0.toml" || true)
[[ -n "$OLD" ]] && kill $OLD && sleep 1
KEEL_OBSERVER_SECRET=$OBS0_SECRET RUST_LOG=${RUST_LOG:-info} nohup "$OBS" run --config "$CFG" >> "$D/observers/0.log" 2>&1 &
say "observer 0 restarted with lightning (pid $!)"
wait_for "invoice API" 60 curl -sf http://127.0.0.1:7201/v1/lightning/info
wait_for "node registered on chain" 60 bash -c "curl -s $RPC/v1/lightning | grep -q '$A_ID'"
say "registered: $(curl -s $RPC/v1/lightning | json "[(p['observer'][:8], p['node_id'][:8], p['balance']) for p in d['pools']]")"

# ---------------------------------------------------------------- pool funding from the vault
POOL=$(curl -s $RPC/v1/lightning | json "int([p for p in d['pools'] if p['observer']=='$OBS0_ADDR'][0]['balance'])")
if (( POOL < 1000000 )); then
  A_FUND_ADDR=$(lncli_ a newaddress p2wkh | json "d['address']")
  "$KEEL" --rpc "$RPC" send --secret "$OBS0_SECRET" --wait 30 lightning fund 3000000 "$A_FUND_ADDR" > "$D/ln-fund.json"
  say "funding outbound: $(json "d.get('error') or 'queued'" < "$D/ln-fund.json")"
  # The vault pays it like any outbound; observers sign and broadcast, we mine.
  wait_for "funding batched and broadcast" 180 bash -c "[ \"\$(bitcoin-cli -regtest -datadir='$D/bitcoind' -rpcport=$BTC_RPC_PORT -rpcuser=keel -rpcpassword=keel getrawmempool | python3 -c 'import sys,json; print(len(json.load(sys.stdin)))')\" -gt 0 ]"
  bcli generatetoaddress 3 "$(bcli -rpcwallet=miner getnewaddress)" >/dev/null
  wait_for "pool credited on chain" 180 bash -c "[ \"\$(curl -s $RPC/v1/lightning | python3 -c 'import sys,json; d=json.load(sys.stdin); print(int([p for p in d[\"pools\"] if p[\"observer\"]==\"$OBS0_ADDR\"][0][\"balance\"]))')\" -ge 3000000 ]"
fi
say "pool: $(curl -s $RPC/v1/lightning | json "[(p['balance'], p['available']) for p in d['pools']]") total=$(curl -s $RPC/v1/lightning | json "d['pool_total']")"

# ---------------------------------------------------------------- deposit: B pays an invoice bound to a chain address
USER_ADDR=$(keyof 1 address)   # seed 1 stands in for a user
BEFORE=$(curl -s "$RPC/v1/accounts/$USER_ADDR" | json "int([b['balance'] for b in d['balances'] if b['asset']=='BTC.BTC' and b['account_type']=='deposit'][0])")
INV=$(curl -sf -X POST http://127.0.0.1:7201/v1/lightning/invoice -H 'content-type: application/json' -d "{\"owner\":\"$USER_ADDR\",\"amount_sat\":25000}" | json "d['payment_request']")
say "invoice for $USER_ADDR: ${INV:0:40}…"
lncli_ b payinvoice --force "$INV" >/dev/null
wait_for "deposit credited on chain" 60 bash -c "[ \"\$(curl -s $RPC/v1/accounts/$USER_ADDR | python3 -c 'import sys,json; d=json.load(sys.stdin); print(int([b[\"balance\"] for b in d[\"balances\"] if b[\"asset\"]==\"BTC.BTC\" and b[\"account_type\"]==\"deposit\"][0]))')\" -ge $((BEFORE + 25000)) ]"
say "deposit: user BTC.BTC $BEFORE → $(curl -s "$RPC/v1/accounts/$USER_ADDR" | json "int([b['balance'] for b in d['balances'] if b['asset']=='BTC.BTC' and b['account_type']=='deposit'][0])") (+25000 sats, credited in seconds)"

# ---------------------------------------------------------------- withdrawal: the chain assigns, LND A pays B's invoice
B_BEFORE=$(lncli_ b channelbalance | json "int(d['local_balance']['sat'])")
B_INV=$(lncli_ b addinvoice --amt 12000 --memo "user withdraw" | json "d['payment_request']")
USER_SECRET=$(keyof 1 secret)
"$KEEL" --rpc "$RPC" send --secret "$USER_SECRET" --wait 30 withdraw BTC.BTC "$B_INV" 12000 > "$D/ln-withdraw.json"
OUT_ID=$(json "[e['WithdrawalQueued']['outbound_id'] for e in d['events'] if 'WithdrawalQueued' in e][0]" < "$D/ln-withdraw.json")
say "withdrawal queued as outbound $OUT_ID, assigned: $(json "[e['LightningPayoutAssigned']['observer'][:8] for e in d['events'] if 'LightningPayoutAssigned' in e]" < "$D/ln-withdraw.json")"
wait_for "payout settled on chain" 90 bash -c "curl -s '$RPC/v1/vaults/outbounds' | python3 -c 'import sys,json; d=json.load(sys.stdin); o=[x for x in d[\"outbounds\"] if x[\"id\"]==$OUT_ID][0]; sys.exit(0 if o[\"status\"]==\"Confirmed\" else 1)'"
B_AFTER=$(lncli_ b channelbalance | json "int(d['local_balance']['sat'])")
say "withdrawal: B's channel balance $B_BEFORE → $B_AFTER (+12000 sats)"
(( B_AFTER == B_BEFORE + 12000 )) || fail "B did not receive 12000 sats"
say "pool after: $(curl -s $RPC/v1/lightning | json "[(p['balance'], p['pending_out']) for p in d['pools']]")"
say "PASS: Lightning deposit (25000 sats, invoice bound to the chain account, credited by the observer's preimage report) and withdrawal (12000 sats to a BOLT11 invoice, assigned to observer 0, paid by its LND, settled on chain)"
