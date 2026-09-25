#!/usr/bin/env bash
# Start / stop / status for the block explorer stack against a local node:
# keel-indexer (explorer API, Postgres) and the apps/explorer Vite dev server.
#
#   infra/dev/explorer.sh start|stop|status
#
# Env: KEEL_NODE_RPC (default http://127.0.0.1:5000), KEEL_NETWORK (testnet),
# INDEXER_DB (postgres://keel:keel@localhost:5434/keel_indexer_testnet, created
# if the keel-test-pg container is running), INDEXER_LISTEN (127.0.0.1:6100),
# EXPLORER_PORT (5177), EXTERNAL_NETWORK (regtest).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
RUN="$ROOT/.dev-run"; LOGS="$RUN/logs"; mkdir -p "$LOGS"
KEEL_NODE_RPC=${KEEL_NODE_RPC:-http://127.0.0.1:5000}
KEEL_NETWORK=${KEEL_NETWORK:-testnet}
INDEXER_DB=${INDEXER_DB:-postgres://keel:keel@localhost:5434/keel_indexer_${KEEL_NETWORK}}
INDEXER_LISTEN=${INDEXER_LISTEN:-127.0.0.1:6100}
EXPLORER_PORT=${EXPLORER_PORT:-5177}
EXTERNAL_NETWORK=${EXTERNAL_NETWORK:-regtest}
BIN="$ROOT/chain/target/release/keel-indexer"

pid_of_port() { ss -ltnp 2>/dev/null | grep ":$1 " | sed -n 's/.*pid=\([0-9]*\).*/\1/p' | head -1; }

ensure_db() {
  local name=${INDEXER_DB##*/}
  local cid; cid=$(docker ps -q --filter publish=5434 2>/dev/null | head -1)
  [ -n "$cid" ] && docker exec "$cid" psql -U keel -d postgres -tc "SELECT 1 FROM pg_database WHERE datname='$name'" 2>/dev/null | grep -q 1 \
    || { [ -n "$cid" ] && docker exec "$cid" psql -U keel -d postgres -c "CREATE DATABASE $name" >/dev/null 2>&1 || true; }
}

start() {
  [ -x "$BIN" ] || (cd "$ROOT/chain" && cargo build --release -p keel-indexer)
  ensure_db
  if [ -f "$RUN/indexer.pid" ] && kill -0 "$(cat "$RUN/indexer.pid")" 2>/dev/null; then
    echo "indexer already running (pid $(cat "$RUN/indexer.pid"))"
  else
    nohup "$BIN" --network "$KEEL_NETWORK" --node-rpc "$KEEL_NODE_RPC" --database-url "$INDEXER_DB" \
      --listen "$INDEXER_LISTEN" --external-network "$EXTERNAL_NETWORK" >> "$LOGS/indexer.log" 2>&1 &
    echo $! > "$RUN/indexer.pid"
    echo "indexer pid $! on http://$INDEXER_LISTEN (log $LOGS/indexer.log)"
  fi
  if [ -n "$(pid_of_port "$EXPLORER_PORT")" ]; then
    echo "explorer already listening on :$EXPLORER_PORT"
  else
    [ -d "$ROOT/apps/explorer/node_modules" ] || (cd "$ROOT/apps/explorer" && npm ci)
    local nets="[{\"id\":\"testnet\",\"name\":\"Testnet\",\"api\":\"http://127.0.0.1:6100\"},{\"id\":\"mainnet\",\"name\":\"Mainnet\",\"api\":\"http://127.0.0.1:6101\"}]"
    (cd "$ROOT/apps/explorer" && VITE_NETWORKS="${VITE_NETWORKS:-$nets}" nohup npx vite --host 127.0.0.1 --port "$EXPLORER_PORT" --strictPort > "$LOGS/explorer.log" 2>&1 &)
    echo "explorer on http://127.0.0.1:$EXPLORER_PORT/$KEEL_NETWORK (log $LOGS/explorer.log)"
  fi
}

stop() {
  if [ -f "$RUN/indexer.pid" ]; then kill "$(cat "$RUN/indexer.pid")" 2>/dev/null || true; rm -f "$RUN/indexer.pid"; fi
  pkill -x keel-indexer 2>/dev/null || true
  local p; p=$(pid_of_port "$EXPLORER_PORT"); [ -n "$p" ] && kill "$p" 2>/dev/null || true
  echo "explorer stack stopped"
}

status() {
  if pgrep -x keel-indexer >/dev/null; then curl -s "http://$INDEXER_LISTEN/v1/health" || echo "indexer up, API not answering"; echo; else echo "indexer: not running"; fi
  [ -n "$(pid_of_port "$EXPLORER_PORT")" ] && echo "explorer: http://127.0.0.1:$EXPLORER_PORT/$KEEL_NETWORK" || echo "explorer: not running"
}

case "${1:-status}" in start) start ;; stop) stop ;; status) status ;; *) echo "usage: $0 start|stop|status"; exit 2 ;; esac
