#!/usr/bin/env bash
# Keel testnet deploy. Run on the box (as the deploy user, passwordless sudo)
# by .github/workflows/deploy-testnet.yml after it unpacked the release into
# ~/keel-deploy and wrote ~/keel-deploy/deploy.env (0600) from the GitHub
# environment's secrets and variables. Idempotent: binaries, config, units,
# web roots and nginx are (re)installed every run; chain state is only
# touched when RESET_CHAIN=true or no genesis exists yet.
set -euo pipefail
D=${KEEL_DEPLOY_DIR:-$HOME/keel-deploy}
ENVF=$D/deploy.env
[ -f "$ENVF" ] || { echo "missing $ENVF"; exit 1; }
set -a; . "$ENVF"; set +a
shred -u "$ENVF" 2>/dev/null || rm -f "$ENVF"

: "${KEEL_VALIDATOR_SEED:?}" "${KEEL_SIGNER_SEED:?}" "${INDEXER_DATABASE_URL:?}" "${BITCOIN_RPC_PASSWORD:?}" "${KEEL_ADVERTISE:?}"
CHAIN_ID=${KEEL_CHAIN_ID:-3}
EXT=${KEEL_EXTERNAL_NETWORK:-signet}
P2P_PORT=${KEEL_P2P_PORT:-3000}; RPC_PORT=${KEEL_RPC_PORT:-5100}; IDX_PORT=${KEEL_INDEXER_PORT:-6100}
RESET=${RESET_CHAIN:-false}
BIN=/opt/keelchain/bin; ETC=/etc/keelchain; LIB=/var/lib/keelchain; WWW=/var/www/keelchain
RPC=http://127.0.0.1:$RPC_PORT

log() { printf '\n== %s\n' "$*"; }
root_write() { # root_write <mode> <path>  (content on stdin)
  sudo install -m "$1" /dev/null "$2"; sudo tee "$2" >/dev/null; }
wait_rpc() { for _ in $(seq 1 60); do curl -sf $RPC/v1/status >/dev/null && return 0; sleep 2; done; echo "node RPC did not come up"; exit 1; }
height() { curl -s $RPC/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["height"])'; }

log "directories and binaries"
sudo install -d -m 755 /opt/keelchain "$BIN" "$LIB" "$WWW"
sudo install -d -m 700 "$ETC"
for b in keel-node keel-indexer keel-observer keel; do sudo install -m 755 "$D/bin/$b" "$BIN/$b"; done
sudo install -m 755 "$D/infra/testnet/btc-checkpoint.sh" /opt/keelchain/btc-checkpoint.sh
sudo install -m 755 "$D/infra/testnet/onboard-client.sh" /opt/keelchain/onboard-client.sh

log "identity"
ACCT=$("$BIN/keel" keygen --seed "$KEEL_VALIDATOR_SEED")
ADDR=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["address"])' "$ACCT")
SECRET=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["secret"])' "$ACCT")
unset ACCT
echo "validator / genesis account: $ADDR"

FRESH=false
if [ "$RESET" != true ] && sudo test -f /opt/keelchain/genesis.json; then
  # The seed must be one of the running chain's validators; otherwise this
  # deploy would start a node the chain does not know. Reset instead.
  if ! sudo python3 -c 'import json,sys; g=json.load(open("/opt/keelchain/genesis.json")); sys.exit(0 if any(v["address"]==sys.argv[1] for v in g["validators"]) else 1)' "$ADDR"; then
    echo "validator $ADDR is not in /opt/keelchain/genesis.json: run the deploy with reset_chain to start a new chain from this seed"; exit 1
  fi
fi
if [ "$RESET" = true ] || ! sudo test -f /opt/keelchain/genesis.json; then
  FRESH=true
  log "bootstrapping a new chain (reset=$RESET)"
  sudo systemctl stop keel-observer@0 keel-indexer keel-validator@0 keel-btc-checkpoint.timer 2>/dev/null || true
  sudo rm -rf "$LIB/validator-0" "$LIB/observer-0.json" "$LIB/vaults-registered"
  "$BIN/keel-node" --devnet --me "$KEEL_VALIDATOR_SEED@$P2P_PORT" --participants "$KEEL_VALIDATOR_SEED" \
    --chain-id "$CHAIN_ID" --external-network "$EXT" --print-genesis --storage-dir /tmp/keel-genesis-probe > /tmp/keel-genesis.json
  python3 - /tmp/keel-genesis.json "$D/infra/testnet/genesis-params.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1])); o = json.load(open(sys.argv[2]))
for k, v in o.items():
    if k.startswith('_'): continue
    if isinstance(v, dict): g['params'][k].update(v)
    else: g['params'][k] = v
json.dump(g, open(sys.argv[1], 'w'), indent=2)
PY
  sudo install -m 644 /tmp/keel-genesis.json /opt/keelchain/genesis.json; rm -f /tmp/keel-genesis.json
  # the indexer's database belongs to the old chain: recreate it
  read -r PGUSER PGDB <<<"$(python3 -c 'import sys; from urllib.parse import urlparse; u=urlparse(sys.argv[1]); print(u.username, u.path.lstrip("/"))' "$INDEXER_DATABASE_URL")"
  if [ -n "${INDEXER_PG_CONTAINER:-}" ]; then
    docker exec "$INDEXER_PG_CONTAINER" psql -U "$PGUSER" -d postgres -c "DROP DATABASE IF EXISTS $PGDB" -c "CREATE DATABASE $PGDB"
  else
    echo "INDEXER_PG_CONTAINER unset: drop and recreate database $PGDB by hand"; fi
fi

log "config"
printf 'SEED=%s\nP2P_PORT=%s\nADVERTISE=%s\nRPC_LISTEN=127.0.0.1\nRPC_PORT=%s\nEXTERNAL_NETWORK=%s\nBOOTSTRAP_ARGS=%s\n' \
  "$KEEL_VALIDATOR_SEED" "$P2P_PORT" "$KEEL_ADVERTISE" "$RPC_PORT" "$EXT" "${KEEL_BOOTSTRAP_ARGS:-}" | root_write 600 "$ETC/validator-0.env"
printf 'KEEL_OBSERVER_SECRET=%s\n' "$SECRET" | root_write 600 "$ETC/observer-0.env"
printf 'DATABASE_URL=%s\nKEEL_NODE_RPC=%s\nKEEL_INDEXER_LISTEN=127.0.0.1:%s\nKEEL_NETWORK=testnet\nKEEL_EXTERNAL_NETWORK=%s\n' \
  "$INDEXER_DATABASE_URL" "$RPC" "$IDX_PORT" "$EXT" | root_write 600 "$ETC/indexer.env"
printf 'BITCOIN_CLI=%s\nCHAIN_ID=%s\nRPC_PORT=%s\n' "${BITCOIN_CLI:-bitcoin-cli}" "$CHAIN_ID" "$RPC_PORT" | root_write 644 "$ETC/checkpoint.env"
echo "$ADDR" | root_write 644 "$ETC/pubkey0"
TRON_KEY_LINE=""; [ -n "${TRONGRID_API_KEY:-}" ] && TRON_KEY_LINE="api_key = \"$TRONGRID_API_KEY\""
python3 - "$D/infra/testnet/observer.toml.tmpl" <<PY | root_write 600 "$ETC/observer-0.toml"
import sys
t = open(sys.argv[1]).read()
for k, v in {
  '@RPC_PORT@': '$RPC_PORT', '@SIGNER_SEED@': '$KEEL_SIGNER_SEED',
  '@BITCOIN_RPC_URL@': '${BITCOIN_RPC_URL:-http://127.0.0.1:38332}', '@BITCOIN_RPC_USER@': '${BITCOIN_RPC_USER:-keel}',
  '@BITCOIN_RPC_PASSWORD@': '$BITCOIN_RPC_PASSWORD', '@EXTERNAL_NETWORK@': '$EXT', '@BITCOIN_WALLET@': '${BITCOIN_WALLET:-keel-vault-watch-0}',
  '@TRON_API_URL@': '${TRON_API_URL:-https://nile.trongrid.io}', '@TRON_API_KEY_LINE@': '$TRON_KEY_LINE',
  '@TRON_USDT_CONTRACT@': '${TRON_USDT_CONTRACT:-TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf}',
}.items(): t = t.replace(k, v)
sys.stdout.write(t)
PY
unset SECRET KEEL_SIGNER_SEED BITCOIN_RPC_PASSWORD INDEXER_DATABASE_URL

log "units"
sudo install -m 644 "$D"/infra/testnet/systemd/* /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable keel-validator@0 keel-indexer keel-observer@0 keel-btc-checkpoint.timer >/dev/null 2>&1
sudo systemctl restart keel-validator@0; wait_rpc
echo "node at height $(height)"

if [ "$FRESH" = true ] || ! sudo test -f "$LIB/vaults-registered"; then
  log "registering vaults (BTC, TRON) with the development signer"
  for chain in BTC TRON; do
    sudo bash -c "set -a; . $ETC/observer-0.env; set +a; $BIN/keel-observer register-vault --config $ETC/observer-0.toml --chain $chain --local-seed \$(sed -n 's/^tss_url = \"local:\\(.*\\)\"/\\1/p' $ETC/observer-0.toml)"
    sleep 8
  done
  sudo touch "$LIB/vaults-registered"
fi
sudo systemctl restart keel-indexer keel-observer@0
sudo systemctl start keel-btc-checkpoint.timer

log "web"
sudo rsync -a --delete "$D/explorer/" "$WWW/explorer/"
sudo rsync -a --delete "$D/site/" "$WWW/site/"
sudo chown -R root:root "$WWW"; sudo find "$WWW" -type d -exec chmod 755 {} + ; sudo find "$WWW" -type f -exec chmod 644 {} +

log "nginx"
if sudo test -f /etc/ssl/keelchain/origin.pem && sudo test -f /etc/ssl/keelchain/origin.key; then
  sudo install -m 644 "$D/infra/testnet/nginx-keelchain.conf" /etc/nginx/sites-available/keelchain
  sudo ln -sf /etc/nginx/sites-available/keelchain /etc/nginx/sites-enabled/keelchain
  [ -f /etc/nginx/conf.d/keel-upgrade-map.conf ] || printf "map \$http_upgrade \$connection_upgrade { default upgrade; '' close; }\n" | root_write 644 /etc/nginx/conf.d/keel-upgrade-map.conf
  sudo nginx -t && sudo systemctl reload nginx
else
  echo "no origin certificate in /etc/ssl/keelchain: nginx config not installed (see README)"
fi

log "health"
sleep 5
curl -s $RPC/v1/status; echo
curl -s http://127.0.0.1:$IDX_PORT/v1/health; echo
systemctl is-active keel-validator@0 keel-indexer keel-observer@0 | tr '\n' ' '; echo
rm -rf "$D"
