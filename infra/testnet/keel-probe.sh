#!/usr/bin/env bash
# Writes this host's chain health as Prometheus metrics for node_exporter's
# textfile collector (every minute, keel-probe.timer): height, oldest block,
# per-network readiness and checkpoint age, pending outbounds, unit states.
# Alert rules in monitoring/alerts.yml read these.
set -uo pipefail
set -a; . /etc/keelchain/checkpoint.env; set +a
N=${HOST_INDEX:-0}
RPC=http://127.0.0.1:${RPC_PORT:-5100}
DIR=/var/lib/node_exporter/textfile
OUT=$DIR/keel.prom.$$
mkdir -p "$DIR"
{
  STATUS=$(curl -sf --max-time 5 "$RPC/v1/status" || echo '{}')
  python3 - "$STATUS" "$N" <<'PY'
import json, sys
s = json.loads(sys.argv[1] or '{}'); host = sys.argv[2]
up = 1 if 'height' in s else 0
print(f'keel_node_up{{host="{host}"}} {up}')
if up:
    print(f'keel_height{{host="{host}"}} {s.get("height", 0)}')
    print(f'keel_oldest_block{{host="{host}"}} {s.get("oldest_block") or 0}')
    print(f'keel_mempool{{host="{host}"}} {s.get("mempool", 0)}')
    print(f'keel_validators{{host="{host}"}} {len(s.get("validators", []))}')
PY
  for chain in BTC ETH TRON; do
    R=$(curl -sf --max-time 5 "$RPC/v1/ready/$chain" || echo '{}')
    python3 - "$R" "$chain" "$N" <<'PY'
import json, sys
r = json.loads(sys.argv[1] or '{}'); chain = sys.argv[2]; host = sys.argv[3]
if not r: sys.exit()
print(f'keel_ready{{host="{host}",chain="{chain}"}} {1 if r.get("ready") else 0}')
print(f'keel_outbound_pending{{host="{host}",chain="{chain}"}} {r.get("outbound_pending", 0)}')
cp = r.get("checkpoint") or {}
if "age_secs" in cp:
    print(f'keel_checkpoint_age_seconds{{host="{host}",chain="{chain}"}} {cp["age_secs"]}')
print(f'keel_vault_registered{{host="{host}",chain="{chain}"}} {1 if r.get("vault") else 0}')
PY
  done
  for unit in "keel-validator@$N" "keel-observer@$N" "keel-tss@$N" keel-indexer bitcoind-signet wg-quick@keel; do
    if systemctl list-unit-files "$unit.service" >/dev/null 2>&1 && systemctl list-unit-files "$unit.service" | grep -q "$unit"; then
      state=$(systemctl is-active "$unit" 2>/dev/null || true)
      [ "$state" = active ] && v=1 || v=0
      restarts=$(systemctl show -p NRestarts --value "$unit" 2>/dev/null || echo 0)
      echo "keel_unit_active{host=\"$N\",unit=\"$unit\"} $v"
      echo "keel_unit_restarts{host=\"$N\",unit=\"$unit\"} ${restarts:-0}"
    fi
  done
  echo "keel_probe_timestamp_seconds{host=\"$N\"} $(date +%s)"
} > "$OUT" 2>/dev/null
mv "$OUT" "$DIR/keel.prom"
