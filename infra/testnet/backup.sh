#!/usr/bin/env bash
# Daily backup of one host: the newest chain snapshot (with its sidecar and
# the epoch boundaries), the root-only config, and on host 0 the indexer
# database. Encrypted with age to AGE_RECIPIENT and copied with rclone to the
# remote named in /etc/keelchain/rclone.conf (`keel-backups:`), keeping 14
# days; without rclone.conf the archives stay under /var/backups/keel.
# Installed by deploy-remote.sh; run by keel-backup.timer as root.
set -euo pipefail
set -a; . /etc/keelchain/checkpoint.env; set +a
N=${HOST_INDEX:-0}
LIB=/var/lib/keelchain; ETC=/etc/keelchain; OUT=/var/backups/keel
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT" "$WORK/state"

VM="$LIB/validator-$N/vm"
NEWEST=$(ls "$VM"/snapshot-*.bin 2>/dev/null | sed -E 's/.*snapshot-([0-9]+)\.bin/\1/' | sort -n | tail -1 || true)
if [ -n "$NEWEST" ]; then
  cp "$VM/snapshot-$NEWEST.bin" "$WORK/state/"
  [ -f "$VM/snapshot-$NEWEST.json" ] && cp "$VM/snapshot-$NEWEST.json" "$WORK/state/"
  [ -f "$VM/boundaries.bin" ] && cp "$VM/boundaries.bin" "$WORK/state/"
fi
[ -f /opt/keelchain/genesis.json ] && cp /opt/keelchain/genesis.json "$WORK/state/"
tar -C "$ETC" -czf "$WORK/etc-keelchain.tgz" .
if [ "$N" = 0 ] && [ -f "$ETC/indexer.env" ]; then
  # shellcheck disable=SC1090
  DBURL=$(sed -n 's/^DATABASE_URL=//p' "$ETC/indexer.env")
  PGC=$(sed -n 's/^INDEXER_PG_CONTAINER=//p' "$ETC/checkpoint.env" 2>/dev/null || true)
  if [ -n "${PGC:-}" ]; then
    docker exec "$PGC" pg_dump --format=custom "$DBURL" > "$WORK/indexer.pgdump" 2>/dev/null || echo "pg_dump failed; continuing without the indexer database"
  fi
fi
ARCHIVE="$OUT/keel-host$N-$STAMP.tar"
tar -C "$WORK" -cf "$ARCHIVE" .
if [ -n "${AGE_RECIPIENT:-}" ] && command -v age >/dev/null; then
  age -r "$AGE_RECIPIENT" -o "$ARCHIVE.age" "$ARCHIVE" && rm -f "$ARCHIVE" && ARCHIVE="$ARCHIVE.age"
fi
chmod 600 "$ARCHIVE"
echo "backup $ARCHIVE ($(du -h "$ARCHIVE" | cut -f1))"
# Local retention: 14 files.
ls -1t "$OUT"/keel-host"$N"-* 2>/dev/null | tail -n +15 | xargs -r rm -f
if [ -f "$ETC/rclone.conf" ] && command -v rclone >/dev/null; then
  rclone --config "$ETC/rclone.conf" copy "$ARCHIVE" "keel-backups:keel/host$N/"
  rclone --config "$ETC/rclone.conf" delete --min-age 14d "keel-backups:keel/host$N/" || true
  echo "copied to keel-backups:keel/host$N/"
fi
