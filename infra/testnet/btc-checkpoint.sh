#!/usr/bin/env bash
# Bitcoin checkpoints for the vault light client, through governance.
#
#   btc-checkpoint.sh propose   host 0, daily: propose SetBtcCheckpoint for
#                               the tip minus 6 (retarget-period aligned),
#                               vote for it, wait for the timelock, execute.
#   btc-checkpoint.sh vote      every host, every 10 minutes: vote Yes on
#                               every open SetBtcCheckpoint proposal whose
#                               header matches this host's view of Bitcoin.
#
# Installed by deploy-remote.sh as /opt/keelchain/btc-checkpoint.sh. Reads
# /etc/keelchain/checkpoint.env (HOST_INDEX, CHAIN_ID, RPC_PORT, BITCOIN_RPC_*)
# and this host's validator account key from /etc/keelchain/observer-N.env.
# Runs as root. Bitcoin is queried over JSON-RPC, so the hosts that share
# host 0's bitcoind over the mesh need no bitcoin-cli.
set -euo pipefail
MODE=${1:-propose}
set -a; . /etc/keelchain/checkpoint.env; set +a
N=${HOST_INDEX:-0}
set -a; . "/etc/keelchain/observer-$N.env"; set +a
RPC=http://127.0.0.1:${RPC_PORT:-5100}
KEEL="/opt/keelchain/bin/keel --rpc $RPC --chain-id ${CHAIN_ID:-3}"
S=$KEEL_OBSERVER_SECRET

btc() { # btc <method> [json args...]
  python3 - "$@" <<'PY'
import json, os, sys, base64, urllib.request
method = sys.argv[1]; params = [json.loads(a) for a in sys.argv[2:]]
req = urllib.request.Request(os.environ["BITCOIN_RPC_URL"], data=json.dumps({"jsonrpc": "1.0", "id": "keel", "method": method, "params": params}).encode())
auth = base64.b64encode(f"{os.environ['BITCOIN_RPC_USER']}:{os.environ['BITCOIN_RPC_PASSWORD']}".encode()).decode()
req.add_header("Authorization", "Basic " + auth); req.add_header("Content-Type", "application/json")
r = json.load(urllib.request.urlopen(req, timeout=30))
if r.get("error"): sys.exit("bitcoin rpc error: " + json.dumps(r["error"]))
v = r["result"]; print(v if isinstance(v, str) else json.dumps(v))
PY
}
height() { curl -s $RPC/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["height"])'; }
net_id() {
  case "$(btc getblockchaininfo | python3 -c 'import json,sys; print(json.load(sys.stdin)["chain"])')" in
    main) echo 0;; test) echo 1;; signet) echo 2;; regtest) echo 3;; *) echo "unknown bitcoin chain"; exit 1;; esac
}
checkpoint_kind() { # checkpoint_kind <height> -> JSON of the proposal kind for this host's view
  local H=$1 START HDR T0
  START=$((H - H % 2016))
  HDR=$(btc getblockheader "\"$(btc getblockhash "$H")\"" false)
  T0=$(btc getblockheader "\"$(btc getblockhash "$START")\"" | python3 -c 'import json,sys; print(json.load(sys.stdin)["time"])')
  python3 -c "import json; print(json.dumps({'SetBtcCheckpoint': {'network': $(net_id), 'height': $H, 'header': list(bytes.fromhex('$HDR')), 'period_start_time': $T0}}, sort_keys=True))"
}

case "$MODE" in
  propose)
    TIP=$(btc getblockcount); H=$((TIP-6))
    KIND=$(checkpoint_kind "$H")
    ID=$($KEEL send --secret "$S" gov propose "Bitcoin checkpoint at $H" "$KIND" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["events"][0]["ProposalCreated"]["proposal_id"])')
    echo "proposal $ID for height $H"; sleep 8
    $KEEL send --secret "$S" gov vote "$ID" Yes >/dev/null && echo "voted"
    END=$(curl -s $RPC/v1/gov/proposals/$ID | python3 -c 'import json,sys; print(json.load(sys.stdin)["timelock_end"])')
    # The other hosts vote through their own timers within ten minutes; the
    # timelock is longer than that on the testnet.
    while [ "$(height)" -le $((END+1)) ]; do sleep 10; done
    $KEEL send --secret "$S" gov execute "$ID" | python3 -c 'import json,sys; d=json.load(sys.stdin); print("executed:", d["ok"], d.get("error"))'
    ;;
  vote)
    # Open SetBtcCheckpoint proposals: vote when the header matches ours.
    curl -s "$RPC/v1/gov/proposals" | python3 -c '
import json, sys
d = json.load(sys.stdin)
items = d.get("proposals", d) if isinstance(d, dict) else d
for p in items:
    kind = p.get("kind") or {}
    if isinstance(kind, dict) and "SetBtcCheckpoint" in kind and str(p.get("status")) == "Voting":
        print(p["id"], kind["SetBtcCheckpoint"]["height"], json.dumps(kind, sort_keys=True, separators=(",", ":")))
' | while read -r ID H THEIRS; do
      MINE=$(checkpoint_kind "$H" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin), sort_keys=True, separators=(",", ":")))')
      if [ "$MINE" = "$THEIRS" ]; then
        OUT=$($KEEL send --secret "$S" gov vote "$ID" Yes 2>&1 || true)
        echo "proposal $ID (height $H): vote -> $(echo "$OUT" | head -c 160)"
      else
        echo "proposal $ID (height $H): header differs from this host's Bitcoin view; not voting"
      fi
    done
    ;;
  *) echo "usage: btc-checkpoint.sh propose|vote"; exit 2;;
esac
