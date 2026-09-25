#!/usr/bin/env bash
# Stop everything the manual testnet started (chain, bitcoind, observers,
# marketplace, backoffice, web). Data under /tmp/keel-custody and the
# keel_testnet database are kept.
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
for p in marketplace backoffice web; do
  [ -f "$ROOT/.dev-run/$p.pid" ] && kill "$(cat "$ROOT/.dev-run/$p.pid")" 2>/dev/null
done
bash "$ROOT/infra/dev/explorer.sh" stop >/dev/null 2>&1
pkill -x keel-observer 2>/dev/null
KEEL_DEVNET_DIR=/tmp/keel-custody bash "$ROOT/infra/dev/devnet.sh" stop >/dev/null 2>&1
bitcoin-cli -regtest -datadir=/tmp/keel-custody/bitcoind -rpcport=18543 -rpcuser=keel -rpcpassword=keel stop >/dev/null 2>&1
echo "testnet stopped"
