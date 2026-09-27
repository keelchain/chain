#!/usr/bin/env bash
# Runs the VM throughput benchmark and fails when it falls below
# KEEL_BENCH_FLOOR actions per second (default 2000: a small runner applies
# signed limit orders far faster than that; the floor catches regressions,
# not absolute performance).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FLOOR=${KEEL_BENCH_FLOOR:-2000}
OUT=$(cd "$ROOT/chain" && cargo bench -p keel-vm --bench throughput 2>&1 | tee /dev/stderr)
RATE=$(echo "$OUT" | grep -E '^apply ' | grep -oE '[0-9]+(\.[0-9]+)? actions/s' | tail -1 | awk '{print $1}')
[[ -n "$RATE" ]] || { echo "benchmark printed no actions/s line"; exit 1; }
python3 -c "import sys; r=float('$RATE'); f=float('$FLOOR'); print(f'throughput {r:.0f} actions/s (floor {f:.0f})'); sys.exit(0 if r >= f else 1)"
