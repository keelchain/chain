#!/usr/bin/env bash
# Restore a host from a backup archive made by backup.sh: stops the units,
# puts the snapshot (and sidecar, boundaries, genesis) back so the node
# resumes from it, restores the indexer database on host 0, starts the
# units. The config under /etc/keelchain is NOT restored: the deploy
# workflow renders it from the GitHub environment.
#
#   restore.sh <archive.tar|archive.tar.age> [age identity file]
set -euo pipefail
ARCHIVE=${1:?archive}; IDENTITY=${2:-}
set -a; . /etc/keelchain/checkpoint.env; set +a
N=${HOST_INDEX:-0}
LIB=/var/lib/keelchain; ETC=/etc/keelchain
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
case "$ARCHIVE" in
  *.age) [ -n "$IDENTITY" ] || { echo "an age identity file is needed for $ARCHIVE"; exit 2; }
         age -d -i "$IDENTITY" -o "$WORK/backup.tar" "$ARCHIVE";;
  *) cp "$ARCHIVE" "$WORK/backup.tar";;
esac
tar -C "$WORK" -xf "$WORK/backup.tar"
systemctl stop "keel-observer@$N" "keel-tss@$N" keel-indexer "keel-validator@$N" 2>/dev/null || true
VM="$LIB/validator-$N/vm"; mkdir -p "$VM"
cp "$WORK"/state/snapshot-*.bin "$VM/" 2>/dev/null || { echo "no snapshot in the archive"; exit 1; }
cp "$WORK"/state/snapshot-*.json "$VM/" 2>/dev/null || true
cp "$WORK"/state/boundaries.bin "$VM/" 2>/dev/null || true
[ -f "$WORK/state/genesis.json" ] && install -m 644 "$WORK/state/genesis.json" /opt/keelchain/genesis.json
if [ "$N" = 0 ] && [ -f "$WORK/indexer.pgdump" ]; then
  DBURL=$(sed -n 's/^DATABASE_URL=//p' "$ETC/indexer.env")
  PGC=$(sed -n 's/^INDEXER_PG_CONTAINER=//p' "$ETC/checkpoint.env" 2>/dev/null || true)
  [ -n "${PGC:-}" ] && docker exec -i "$PGC" pg_restore --clean --if-exists --dbname="$DBURL" < "$WORK/indexer.pgdump" || echo "indexer restore skipped"
fi
systemctl start "keel-validator@$N"
sleep 5
systemctl start "keel-observer@$N" 2>/dev/null || true
[ "$N" = 0 ] && systemctl start keel-indexer
curl -s http://127.0.0.1:${RPC_PORT:-5100}/v1/status | head -c 300; echo
echo "restored from $ARCHIVE"
