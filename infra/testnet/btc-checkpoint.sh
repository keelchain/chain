#!/usr/bin/env bash
# Propose, vote and execute a SetBtcCheckpoint for the Bitcoin tip minus 6,
# aligned to the retarget period. Installed by deploy-remote.sh as
# /opt/keelchain/btc-checkpoint.sh and run by keel-btc-checkpoint.timer.
# Reads /etc/keelchain/checkpoint.env (BITCOIN_CLI, CHAIN_ID, RPC_PORT) and
# the governance key from /etc/keelchain/observer-0.env. Runs as root.
set -euo pipefail
set -a; . /etc/keelchain/checkpoint.env; . /etc/keelchain/observer-0.env; set +a
RPC=http://127.0.0.1:${RPC_PORT:-5100}
KEEL="/opt/keelchain/bin/keel --rpc $RPC --chain-id ${CHAIN_ID:-3}"
CLI=$BITCOIN_CLI
S=$KEEL_OBSERVER_SECRET
NET=$($CLI getblockchaininfo | python3 -c 'import json,sys; print(json.load(sys.stdin)["chain"])')
case "$NET" in main) N=0;; test) N=1;; signet) N=2;; regtest) N=3;; *) echo "unknown bitcoin chain $NET"; exit 1;; esac
TIP=$($CLI getblockcount); H=$((TIP-6)); START=$((H - H % 2016))
HDR=$($CLI getblockheader "$($CLI getblockhash $H)" false)
T0=$($CLI getblockheader "$($CLI getblockhash $START)" | python3 -c 'import json,sys; print(json.load(sys.stdin)["time"])')
KIND=$(python3 -c "import json; print(json.dumps({'SetBtcCheckpoint': {'network': $N, 'height': $H, 'header': list(bytes.fromhex('$HDR')), 'period_start_time': $T0}}))")
ID=$($KEEL send --secret "$S" gov propose "Bitcoin checkpoint at $H" "$KIND" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["events"][0]["ProposalCreated"]["proposal_id"])')
echo "proposal $ID for height $H"; sleep 8
$KEEL send --secret "$S" gov vote "$ID" Yes >/dev/null
END=$(curl -s $RPC/v1/gov/proposals/$ID | python3 -c 'import json,sys; print(json.load(sys.stdin)["timelock_end"])')
while [ "$(curl -s $RPC/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["height"])')" -le $((END+1)) ]; do sleep 5; done
$KEEL send --secret "$S" gov execute "$ID" | python3 -c 'import json,sys; d=json.load(sys.stdin); print("executed:", d["ok"], d.get("error"))'
