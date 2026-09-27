#!/usr/bin/env bash
# This host's validator votes Yes, every ten minutes (keel-vote.timer), on
# proposals I have pre-approved in writing on this host:
#   - SetBtcCheckpoint whose header matches this host's view of Bitcoin
#     (btc-checkpoint.sh vote);
#   - SetAttesters / SetParamAdmin naming an address listed in
#     /etc/keelchain/clients.json (the KEEL_CLIENTS variable);
#   - SoftwareUpgrade whose version is listed in
#     /etc/keelchain/upgrades-approved (written by the propose-upgrade run).
# Anything else waits for a hand vote. Voting twice is refused by the chain
# and harmless.
set -uo pipefail
set -a; . /etc/keelchain/checkpoint.env; set +a
N=${HOST_INDEX:-0}
set -a; . "/etc/keelchain/observer-$N.env"; set +a
RPC=http://127.0.0.1:${RPC_PORT:-5100}
KEEL="/opt/keelchain/bin/keel --rpc $RPC --chain-id ${CHAIN_ID:-3}"
S=$KEEL_OBSERVER_SECRET
/opt/keelchain/btc-checkpoint.sh vote || true
CLIENTS=$(cat /etc/keelchain/clients.json 2>/dev/null || echo '[]')
APPROVED=$(cat /etc/keelchain/upgrades-approved 2>/dev/null || true)
curl -s "$RPC/v1/gov/proposals" | python3 -c '
import json, sys
clients = {c.get("address") for c in json.loads(sys.argv[1] or "[]")}
approved = set(sys.argv[2].split())
d = json.load(sys.stdin)
items = d.get("proposals", d) if isinstance(d, dict) else d
for p in items:
    if str(p.get("status")) != "Voting": continue
    kind = p.get("kind") or {}
    if not isinstance(kind, dict): continue
    if "SetAttesters" in kind:
        members = set(kind["SetAttesters"].get("members", []))
        # The new set may only add listed clients to the current attesters.
        if members and members <= (clients | set(sys.argv[3].split())):
            print(p["id"], "attesters")
    elif "SetParamAdmin" in kind and kind["SetParamAdmin"].get("admin") in clients:
        print(p["id"], "param-admin")
    elif "SoftwareUpgrade" in kind and kind["SoftwareUpgrade"].get("version") in approved:
        print(p["id"], "upgrade")
' "$CLIENTS" "$APPROVED" "$(curl -s "$RPC/v1/roles" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(" ".join(d.get("attesters", [])))' 2>/dev/null)" | while read -r ID WHY; do
  OUT=$($KEEL send --secret "$S" gov vote "$ID" Yes 2>&1 || true)
  echo "proposal $ID ($WHY): vote -> $(echo "$OUT" | head -c 120)"
done
