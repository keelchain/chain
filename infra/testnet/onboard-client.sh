#!/usr/bin/env bash
# Onboard a client account on the Keel testnet, run on the box by
# .github/workflows/onboard-client.yml (as ubuntu, sudo for the key).
#   onboard-client.sh <address> [--attester] [--param-admin] [--arbitrator <addr>] [--keel N] [--kusd N]
# `--arbitrator` registers a second address (the client's dispute-ruling
# key) in the arbitrator set.
# Roles go through governance (propose, vote with the genesis validator, wait
# for the timelock, execute); funds are plain transfers from the genesis account.
set -euo pipefail
ADDR=${1:?address}; shift
ATTESTER=0; PARAM_ADMIN=0; ARBITRATOR=""; KEEL_AMT=0; KUSD_AMT=0
while [ $# -gt 0 ]; do case "$1" in
  --attester) ATTESTER=1;; --param-admin) PARAM_ADMIN=1;; --arbitrator) ARBITRATOR=$2; shift;;
  --keel) KEEL_AMT=$2; shift;; --kusd) KUSD_AMT=$2; shift;; *) echo "unknown $1"; exit 2;; esac; shift; done
[[ "$ADDR" =~ ^[0-9a-f]{64}$ ]] || { echo "address must be 64 hex"; exit 2; }
set -a; . <(sudo cat /etc/keelchain/checkpoint.env); set +a
RPC=http://127.0.0.1:${RPC_PORT:-5100}
KEEL="/opt/keelchain/bin/keel --rpc $RPC --chain-id ${CHAIN_ID:-3}"
S=$(sudo sed -n 's/^KEEL_OBSERVER_SECRET=//p' /etc/keelchain/observer-0.env)
ME=$(sudo cat /etc/keelchain/pubkey0)
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
height() { curl -s $RPC/v1/status | j 'd["height"]'; }
roles() { curl -s $RPC/v1/gov/roles; }
send() { $KEEL send --secret "$S" "$@"; }
propose() { # title kind -> id
  local out; out=$(send gov propose "$1" "$2")
  echo "$out" | j 'd["events"][0]["ProposalCreated"]["proposal_id"]' || { echo "$out" >&2; exit 1; }
}
# One proposal deposit must fit the genesis account; lower it while this key is param admin.
DEP=$(curl -s $RPC/v1/params | j 'd["proposal_deposit"]')
LIQ=$($KEEL account "$ME" | j '[b["balance"] for b in d["balances"] if b["asset"]=="KEEL" and b["account_type"]=="deposit"][0]')
if [ "$DEP" -gt "$((LIQ/2))" ] && [ "$(roles | j 'd["param_admin"]')" = "$ME" ]; then
  send raw '{"SetParam":{"key":"proposal_deposit","value":1000000000}}' >/dev/null; echo "proposal_deposit lowered to 1000 KEEL"; sleep 6
fi
IDS=()
if [ $ATTESTER = 1 ]; then
  MEMBERS=$(roles | python3 -c "import json,sys; m=json.load(sys.stdin)['attesters']; m=m if '$ADDR' in m else m+['$ADDR']; print(json.dumps({'SetAttesters':{'members':m}}))")
  IDS+=("$(propose "Attester $ADDR" "$MEMBERS")"); sleep 6
fi
if [ $PARAM_ADMIN = 1 ]; then
  IDS+=("$(propose "Param admin $ADDR" "{\"SetParamAdmin\":{\"admin\":\"$ADDR\"}}")"); sleep 6
fi
if [ -n "$ARBITRATOR" ]; then
  [[ "$ARBITRATOR" =~ ^[0-9a-f]{64}$ ]] || { echo "arbitrator address must be 64 hex"; exit 2; }
  MEMBERS=$(roles | python3 -c "import json,sys; m=json.load(sys.stdin)['arbitrators']; m=m if '$ARBITRATOR' in m else m+['$ARBITRATOR']; print(json.dumps({'SetArbitrators':{'members':m}}))")
  IDS+=("$(propose "Arbitrator $ARBITRATOR" "$MEMBERS")"); sleep 6
fi
for ID in "${IDS[@]}"; do send gov vote "$ID" Yes >/dev/null; echo "proposal $ID voted"; sleep 6; done
[ "$KEEL_AMT" != 0 ] && { send transfer "$ADDR" KEEL "$((KEEL_AMT*1000000))" >/dev/null; echo "sent $KEEL_AMT KEEL"; sleep 6; }
[ "$KUSD_AMT" != 0 ] && { send transfer "$ADDR" KUSD "$((KUSD_AMT*1000000))" >/dev/null; echo "sent $KUSD_AMT KUSD"; sleep 6; }
for ID in "${IDS[@]}"; do
  END=$(curl -s $RPC/v1/gov/proposals/$ID | j 'd["timelock_end"]')
  echo "proposal $ID: waiting for timelock end $END (height $(height))"
  while [ "$(height)" -le $((END+1)) ]; do sleep 5; done
  send gov execute "$ID" | j '"executed:", d["ok"], d.get("error")'
done
echo "roles now:"; roles; echo; $KEEL account "$ADDR"
