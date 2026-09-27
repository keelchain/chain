#!/usr/bin/env bash
# Reads the last N days from the host-0 Prometheus (monitoring/compose) and
# prints what a soak must show: the longest gap between height increments,
# readiness dips per network, the oldest checkpoint age, unit restarts.
#   soak-report.sh [days] [prometheus url]
set -euo pipefail
DAYS=${1:-7}; PROM=${2:-http://127.0.0.1:9090}
q() { curl -sf --get "$PROM/api/v1/query" --data-urlencode "query=$1" | python3 -c 'import json,sys; d=json.load(sys.stdin)["data"]["result"]; print("\n".join(f"{r[\"metric\"]} {r[\"value\"][1]}" for r in d) if d else "-")'; }
echo "== soak report, last $DAYS d"
echo "-- longest stall (seconds without a new height), per host"
q "max_over_time((time() - timestamp(changes(keel_height[5m]) > bool 0))[${DAYS}d:1m])"
echo "-- minutes with keel_node_up == 0"
q "count_over_time((keel_node_up == 0)[${DAYS}d:1m])"
echo "-- minutes not ready, per chain"
q "count_over_time((keel_ready == 0)[${DAYS}d:1m])"
echo "-- oldest checkpoint age seen"
q "max_over_time(keel_checkpoint_age_seconds[${DAYS}d])"
echo "-- unit restarts"
q "max_over_time(keel_unit_restarts[${DAYS}d]) - min_over_time(keel_unit_restarts[${DAYS}d])"
echo "-- pending outbounds, max"
q "max_over_time(keel_outbound_pending[${DAYS}d])"
