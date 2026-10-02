#!/usr/bin/env bash
# Keel testnet deploy, one host of the set. Run on the box (as the deploy
# user, passwordless sudo) by .github/workflows/deploy-testnet.yml after it
# unpacked the release into ~/keel-deploy and wrote ~/keel-deploy/deploy.env
# (0600) from the GitHub environment's secrets and variables for THIS host.
#
# Host layout (HOST_INDEX in HOSTS_JSON):
#   every host   keel-validator@N, keel-observer@N, keel-tss@N (signer mode
#                tss), the checkpoint vote timer, WireGuard mesh `keel`
#   host 0 also  keel-indexer, nginx + web roots, the checkpoint proposal
#                timer, the shared signet bitcoind (installed by prepare-host)
#
# Idempotent: binaries, config, units, web roots and nginx are (re)installed
# every run; chain state is only touched when RESET_CHAIN=true or no genesis
# exists yet. The genesis file itself comes from the release (built by the
# workflow's genesis job from public keys), never from this host's seed.
set -euo pipefail
D=${KEEL_DEPLOY_DIR:-$HOME/keel-deploy}
ENVF=$D/deploy.env
[ -f "$ENVF" ] || { echo "missing $ENVF"; exit 1; }
set -a; . "$ENVF"; set +a
shred -u "$ENVF" 2>/dev/null || rm -f "$ENVF"

: "${HOST_INDEX:?}" "${HOSTS_JSON:?}" "${KEEL_VALIDATOR_SEED:?}"
N=$HOST_INDEX
CHAIN_ID=${KEEL_CHAIN_ID:-3}
EXT=${KEEL_EXTERNAL_NETWORK:-signet}
SIGNER_MODE=${KEEL_SIGNER_MODE:-tss}
P2P_PORT=${KEEL_P2P_PORT:-3000}; RPC_PORT=${KEEL_RPC_PORT:-5100}; IDX_PORT=${KEEL_INDEXER_PORT:-6100}
TSS_PORT=7000; TSS_HTTP=7100
RESET=${RESET_CHAIN:-false}
BIN=/opt/keelchain/bin; ETC=/etc/keelchain; LIB=/var/lib/keelchain; WWW=/var/www/keelchain
RPC=http://127.0.0.1:$RPC_PORT

log() { printf '\n== %s\n' "$*"; }
root_write() { # root_write <mode> <path>  (content on stdin)
  sudo install -m "$1" /dev/null "$2"; sudo tee "$2" >/dev/null; }
wait_rpc() {
  for _ in $(seq 1 90); do curl -sf $RPC/v1/status >/dev/null && return 0; sleep 2; done
  echo "node RPC did not come up; unit state and the last log lines:"
  sudo systemctl status "keel-validator@$N" --no-pager -l 2>&1 | head -20
  sudo journalctl -u "keel-validator@$N" -n 80 --no-pager 2>&1 | tail -80
  exit 1
}
height() { curl -s $RPC/v1/status | python3 -c 'import json,sys; print(json.load(sys.stdin)["height"])'; }
hostf() { # hostf <index> <field>
  python3 -c 'import json,sys; print(json.loads(sys.argv[1])[int(sys.argv[2])].get(sys.argv[3], ""))' "$HOSTS_JSON" "$1" "$2"; }
NHOSTS=$(python3 -c 'import json,sys; print(len(json.loads(sys.argv[1])))' "$HOSTS_JSON")
MY_IP=$(hostf "$N" ip); MY_WG=$(hostf "$N" wg_ip)
IS_HOST0=false; [ "$N" = 0 ] && IS_HOST0=true

log "host $N of $NHOSTS ($MY_IP, wg ${MY_WG:-none}), signer mode $SIGNER_MODE, reset_chain=$RESET"

log "directories and binaries"
sudo install -d -m 755 /opt/keelchain "$BIN" "$LIB" "$LIB/tss"
sudo install -d -m 700 "$ETC"
for b in keel-node keel-indexer keel-observer keel keel-tss; do sudo install -m 755 "$D/bin/$b" "$BIN/$b"; done
sudo install -m 755 "$D/infra/testnet/btc-checkpoint.sh" /opt/keelchain/btc-checkpoint.sh
sudo install -m 755 "$D/infra/testnet/onboard-client.sh" /opt/keelchain/onboard-client.sh
sudo install -m 755 "$D/infra/testnet/tss-ceremony.sh" /opt/keelchain/tss-ceremony.sh
for s in backup.sh restore.sh keel-probe.sh keel-vote.sh soak-report.sh; do sudo install -m 755 "$D/infra/testnet/$s" "/opt/keelchain/$s"; done

log "identity"
ACCT=$("$BIN/keel" keygen --seed "$KEEL_VALIDATOR_SEED")
ADDR=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["address"])' "$ACCT")
SECRET=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["secret"])' "$ACCT")
unset ACCT
EXPECTED=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[int(sys.argv[2])]["address"])' "$D/identities.json" "$N")
[ "$ADDR" = "$EXPECTED" ] || { echo "this host's seed derives $ADDR but the release expects $EXPECTED for host $N"; exit 1; }
echo "validator / observer account: $ADDR"

if [ "$NHOSTS" -gt 1 ] && [ -n "${WG_PRIVATE_KEY:-}" ]; then
  log "wireguard mesh"
  PEERS=$(python3 - "$HOSTS_JSON" "$WG_PUBKEYS" "$N" <<'PY'
import json, sys
hosts = json.loads(sys.argv[1]); pubs = json.loads(sys.argv[2]); me = int(sys.argv[3])
for i, h in enumerate(hosts):
    if i == me: continue
    print(f"[Peer]\nPublicKey = {pubs[i]}\nAllowedIPs = {h['wg_ip']}/32\nEndpoint = {h['ip']}:51820\nPersistentKeepalive = 25\n")
PY
)
  printf '[Interface]\nAddress = %s/24\nListenPort = 51820\nPrivateKey = %s\n\n%s' "$MY_WG" "$WG_PRIVATE_KEY" "$PEERS" | root_write 600 /etc/wireguard/keel.conf
  sudo systemctl enable wg-quick@keel >/dev/null 2>&1 || true
  sudo systemctl restart wg-quick@keel
fi
unset WG_PRIVATE_KEY

FRESH=false
if [ "$RESET" != true ] && sudo test -f /opt/keelchain/genesis.json; then
  if ! sudo cmp -s "$D/genesis.json" /opt/keelchain/genesis.json; then
    echo "the release genesis differs from /opt/keelchain/genesis.json: the validator set or the parameters changed; run the deploy with reset_chain"; exit 1
  fi
fi
if [ "$RESET" = true ] || ! sudo test -f /opt/keelchain/genesis.json; then
  FRESH=true
  log "bootstrapping a new chain (reset=$RESET)"
  sudo systemctl stop "keel-observer@$N" "keel-tss@$N" "keel-validator@$N" keel-indexer keel-btc-checkpoint.timer keel-btc-checkpoint-vote.timer keel-vote.timer keel-probe.timer keel-backup.timer 2>/dev/null || true
  sudo rm -rf "$LIB/validator-$N" "$LIB/observer-$N.json" "$LIB/vaults-registered"
  sudo install -m 644 "$D/genesis.json" /opt/keelchain/genesis.json
  if [ "$IS_HOST0" = true ]; then
    # the indexer's database belongs to the old chain: recreate it
    read -r PGUSER PGDB <<<"$(python3 -c 'import sys; from urllib.parse import urlparse; u=urlparse(sys.argv[1]); print(u.username, u.path.lstrip("/"))' "${INDEXER_DATABASE_URL:?}")"
    if [ -n "${INDEXER_PG_CONTAINER:-}" ]; then
      docker exec "$INDEXER_PG_CONTAINER" psql -U "$PGUSER" -d postgres -c "DROP DATABASE IF EXISTS $PGDB" -c "CREATE DATABASE $PGDB"
    else
      echo "INDEXER_PG_CONTAINER unset: drop and recreate database $PGDB by hand"; fi
  fi
fi

log "config"
BOOT=$(python3 - "$HOSTS_JSON" "$D/identities.json" "$N" "$P2P_PORT" <<'PY'
import json, sys
hosts = json.loads(sys.argv[1]); ids = json.load(open(sys.argv[2])); me = int(sys.argv[3]); port = sys.argv[4]
peers = [f"{ids[i]['consensus_key']}@{h['ip']}:{port}" for i, h in enumerate(hosts) if i != me]
print("--bootstrappers " + ",".join(peers) if peers else "")
PY
)
printf 'SEED=%s\nP2P_PORT=%s\nADVERTISE=%s:%s\nRPC_LISTEN=127.0.0.1\nRPC_PORT=%s\nEXTERNAL_NETWORK=%s\nBOOTSTRAP_ARGS=%s\nEXTRA_ARGS=%s\n' \
  "$KEEL_VALIDATOR_SEED" "$P2P_PORT" "$MY_IP" "$P2P_PORT" "$RPC_PORT" "$EXT" "$BOOT" "${KEEL_NODE_EXTRA_ARGS:-}" | root_write 600 "$ETC/validator-$N.env"
printf 'KEEL_OBSERVER_SECRET=%s\n' "$SECRET" | root_write 600 "$ETC/observer-$N.env"
if [ "$IS_HOST0" = true ]; then
  # The faucet key: the KEEL_FAUCET_SECRET secret, or one generated on the
  # box (`/etc/keelchain/faucet.env`, root-only) so the key never leaves it.
  FAUCET=${KEEL_FAUCET_SECRET:-$(sudo sed -n 's/^KEEL_FAUCET_SECRET=//p' "$ETC/faucet.env" 2>/dev/null || true)}
  printf 'DATABASE_URL=%s\nKEEL_NODE_RPC=%s\nKEEL_INDEXER_LISTEN=127.0.0.1:%s\nKEEL_NETWORK=testnet\nKEEL_EXTERNAL_NETWORK=%s\nKEEL_FAUCET_SECRET=%s\nKEEL_FAUCET_KEEL=%s\nKEEL_FAUCET_KUSD=%s\nKEEL_FAUCET_COOLDOWN_SECS=%s\nKEEL_FAUCET_IP_PER_DAY=%s\nKEEL_EXPLORER_URL=%s\n' \
    "${INDEXER_DATABASE_URL:?}" "$RPC" "$IDX_PORT" "$EXT" "$FAUCET" "${KEEL_FAUCET_KEEL:-100}" "${KEEL_FAUCET_KUSD:-100}" "${KEEL_FAUCET_COOLDOWN_SECS:-86400}" "${KEEL_FAUCET_IP_PER_DAY:-5}" "${KEEL_EXPLORER_URL:-https://testnet.keelchain.com}" | root_write 600 "$ETC/indexer.env"
  unset FAUCET KEEL_FAUCET_SECRET
fi
# Bitcoin RPC: host 0 runs bitcoind on loopback and on its WireGuard address;
# the other hosts reach it over the mesh.
if [ "$IS_HOST0" = true ] || [ "$NHOSTS" = 1 ]; then BTC_URL=${BITCOIN_RPC_URL:-http://127.0.0.1:38332}
else BTC_URL="http://$(hostf 0 wg_ip):38332"; fi
printf 'HOST_INDEX=%s\nCHAIN_ID=%s\nRPC_PORT=%s\nBITCOIN_RPC_URL=%s\nBITCOIN_RPC_USER=%s\nBITCOIN_RPC_PASSWORD=%s\n' \
  "$N" "$CHAIN_ID" "$RPC_PORT" "$BTC_URL" "${BITCOIN_RPC_USER:-keel}" "${BITCOIN_RPC_PASSWORD:?}" | root_write 600 "$ETC/checkpoint.env"
echo "$ADDR" | root_write 644 "$ETC/pubkey$N"
# Pre-approved votes: the clients I onboard (KEEL_CLIENTS) and backups.
printf '%s\n' "${KEEL_CLIENTS:-[]}" | root_write 644 "$ETC/clients.json"
printf 'AGE_RECIPIENT=%s\nINDEXER_PG_CONTAINER=%s\n' "${AGE_RECIPIENT:-}" "${INDEXER_PG_CONTAINER:-}" | root_write 600 "$ETC/backup.env"
printf 'INDEXER_PG_CONTAINER=%s\n' "${INDEXER_PG_CONTAINER:-}" | sudo tee -a "$ETC/checkpoint.env" >/dev/null
if [ -n "${RCLONE_CONF:-}" ]; then printf '%s\n' "$RCLONE_CONF" | root_write 600 "$ETC/rclone.conf"; fi
unset RCLONE_CONF
# journald: a bounded log, not a full disk.
printf '[Journal]\nSystemMaxUse=500M\nMaxRetentionSec=14day\n' | root_write 644 /etc/systemd/journald.conf.d/keel.conf 2>/dev/null || { sudo install -d -m 755 /etc/systemd/journald.conf.d; printf '[Journal]\nSystemMaxUse=500M\nMaxRetentionSec=14day\n' | root_write 644 /etc/systemd/journald.conf.d/keel.conf; }
sudo systemctl restart systemd-journald 2>/dev/null || true
case "$SIGNER_MODE" in
  tss)   TSS_URL="http://127.0.0.1:$TSS_HTTP";;
  local) : "${KEEL_SIGNER_SEED:?local signer mode needs KEEL_SIGNER_SEED}"; TSS_URL="local:$KEEL_SIGNER_SEED";;
  *) echo "KEEL_SIGNER_MODE must be tss or local"; exit 1;;
esac
TRON_KEY_LINE=""; [ -n "${TRONGRID_API_KEY:-}" ] && TRON_KEY_LINE="api_key = \"$TRONGRID_API_KEY\""
python3 - "$D/infra/testnet/observer.toml.tmpl" <<PY | root_write 600 "$ETC/observer-$N.toml"
import sys
t = open(sys.argv[1]).read()
for k, v in {
  '@N@': '$N', '@RPC_PORT@': '$RPC_PORT', '@TSS_URL@': '$TSS_URL',
  '@BITCOIN_RPC_URL@': '$BTC_URL', '@BITCOIN_RPC_USER@': '${BITCOIN_RPC_USER:-keel}',
  '@BITCOIN_RPC_PASSWORD@': '$BITCOIN_RPC_PASSWORD', '@EXTERNAL_NETWORK@': '$EXT', '@BITCOIN_WALLET@': '${BITCOIN_WALLET:-keel-vault-watch}-$N',
  '@TRON_API_URL@': '${TRON_API_URL:-https://nile.trongrid.io}', '@TRON_API_KEY_LINE@': '$TRON_KEY_LINE',
  '@TRON_USDT_CONTRACT@': '${TRON_USDT_CONTRACT:-TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf}',
}.items(): t = t.replace(k, v)
sys.stdout.write(t)
PY
if [ "$SIGNER_MODE" = tss ]; then
  : "${KEEL_TSS_PASSPHRASE:?}" "${KEEL_TSS_SECRET:?}"
  TSS_PEERS=$(python3 -c 'import json,sys; hs=json.loads(sys.argv[1]); print(",".join(f"{h.get(\"wg_ip\") or h[\"ip\"]}:{sys.argv[2]}" for h in hs))' "$HOSTS_JSON" "$TSS_PORT")
  THRESHOLD=${KEEL_OBSERVER_THRESHOLD:-$(( (NHOSTS * 2 + 2) / 3 ))}; [ "$THRESHOLD" -lt 1 ] && THRESHOLD=1
  printf 'KEEL_TSS_PASSPHRASE=%s\nKEEL_TSS_SECRET=%s\nTSS_INDEX=%s\nTSS_N=%s\nTSS_T=%s\nTSS_PEERS=%s\nTSS_HTTP=127.0.0.1:%s\n' \
    "$KEEL_TSS_PASSPHRASE" "$KEEL_TSS_SECRET" "$N" "$NHOSTS" "$THRESHOLD" "$TSS_PEERS" "$TSS_HTTP" | root_write 600 "$ETC/tss-$N.env"
fi
unset SECRET KEEL_SIGNER_SEED BITCOIN_RPC_PASSWORD INDEXER_DATABASE_URL KEEL_TSS_PASSPHRASE KEEL_TSS_SECRET

log "units"
sudo install -m 644 "$D"/infra/testnet/systemd/* /etc/systemd/system/
sudo systemctl daemon-reload
UNITS="keel-validator@$N keel-observer@$N keel-vote.timer keel-probe.timer keel-backup.timer"
[ "$SIGNER_MODE" = tss ] && UNITS="$UNITS keel-tss@$N"
[ "$IS_HOST0" = true ] && UNITS="$UNITS keel-indexer keel-btc-checkpoint.timer"
# shellcheck disable=SC2086
sudo systemctl enable $UNITS >/dev/null 2>&1
sudo systemctl restart "keel-validator@$N"; wait_rpc
echo "node at height $(height)"

if [ "$SIGNER_MODE" = local ] && [ "$IS_HOST0" = true ] && { [ "$FRESH" = true ] || ! sudo test -f "$LIB/vaults-registered"; }; then
  log "registering vaults (BTC, TRON) with the development signer"
  sleep 10
  for chain in BTC TRON; do
    sudo bash -c "set -a; . $ETC/observer-$N.env; set +a; $BIN/keel-observer register-vault --config $ETC/observer-$N.toml --chain $chain --local-seed \$(sed -n 's/^tss_url = \"local:\\(.*\\)\"/\\1/p' $ETC/observer-$N.toml)"
    sleep 8
  done
  sudo touch "$LIB/vaults-registered"
elif [ "$SIGNER_MODE" = tss ] && [ "$FRESH" = true ]; then
  echo "fresh chain in tss mode: run the 'TSS ceremony' workflow to create the vault key and register the vaults"
fi
if [ "$SIGNER_MODE" = tss ]; then
  # Starts only once the ceremony wrote the share (ConditionPathExists).
  sudo systemctl restart "keel-tss@$N" 2>/dev/null || true
fi
sudo systemctl restart "keel-observer@$N"
sudo systemctl start keel-vote.timer keel-probe.timer keel-backup.timer
sudo systemctl stop keel-btc-checkpoint-vote.timer 2>/dev/null || true; sudo systemctl disable keel-btc-checkpoint-vote.timer 2>/dev/null || true
if [ "$IS_HOST0" = true ]; then
  sudo systemctl restart keel-indexer
  sudo systemctl start keel-btc-checkpoint.timer
  if [ "$FRESH" = true ] && [ "$SIGNER_MODE" = local ]; then
    # Deposits verify against a Bitcoin checkpoint; propose the first one now
    # (the oneshot waits for the vote and timelock, so do not block on it).
    sudo systemctl start --no-block keel-btc-checkpoint.service
  fi
  if [ "$FRESH" = true ] && [ "${KEEL_CLIENTS:-[]}" != "[]" ]; then
    # Re-onboard every listed client: each run proposes, votes with this
    # host, waits for the other hosts' vote timers and the timelock, and
    # executes; they run in the background, one after the other.
    log "re-onboarding clients after the reset"
    python3 -c 'import json,sys
for c in json.loads(sys.argv[1]):
    a=["--attester"] if c.get("attester", True) else []
    a+=["--param-admin"] if c.get("param_admin") else []
    a+=["--keel", str(c.get("keel", 100000)), "--kusd", str(c.get("kusd", 100000))]
    print(c["address"], " ".join(a))' "$KEEL_CLIENTS" | while read -r ADDRC ARGSC; do
      # shellcheck disable=SC2086
      sudo systemd-run --unit "keel-onboard-${ADDRC:0:8}" --collect --quiet /opt/keelchain/onboard-client.sh "$ADDRC" $ARGSC || true
    done
  fi
  if [ "${KEEL_MONITORING:-0}" = 1 ]; then
    log "monitoring stack"
    sudo install -d -m 755 /opt/keelchain/monitoring
    sudo install -m 644 "$D/infra/testnet/monitoring/compose.yml" "$D/infra/testnet/monitoring/alerts.yml" "$D/infra/testnet/monitoring/grafana-datasource.yml" /opt/keelchain/monitoring/
    TARGETS=$(python3 -c 'import json,sys; hs=json.loads(sys.argv[1]); print(", ".join("\"%s:9100\"" % (h.get("wg_ip") or "127.0.0.1") for h in hs))' "$HOSTS_JSON")
    sed "s|@NODE_EXPORTER_TARGETS@|$TARGETS|" "$D/infra/testnet/monitoring/prometheus.yml.tmpl" | root_write 644 /opt/keelchain/monitoring/prometheus.yml
    python3 - "$D/infra/testnet/monitoring/alertmanager.yml.tmpl" <<PY | root_write 600 /opt/keelchain/monitoring/alertmanager.yml
import sys
t = open(sys.argv[1]).read()
for k, v in {'@SMTP_HOST@': '${ALERT_SMTP_HOST:-localhost:25}', '@SMTP_FROM@': '${ALERT_SMTP_FROM:-alerts@keelchain.com}', '@SMTP_USER@': '${ALERT_SMTP_USER:-}', '@SMTP_PASSWORD@': '${ALERT_SMTP_PASSWORD:-}', '@ALERT_TO@': '${ALERT_TO:-support@keelchain.com}'}.items(): t = t.replace(k, v)
sys.stdout.write(t)
PY
    printf 'GRAFANA_PASSWORD=%s\n' "${GRAFANA_PASSWORD:-admin}" | root_write 600 /opt/keelchain/monitoring/.env
    (cd /opt/keelchain/monitoring && sudo docker compose --env-file .env up -d --remove-orphans) || echo "monitoring stack did not start"
  fi
  unset ALERT_SMTP_PASSWORD GRAFANA_PASSWORD
fi

if [ "$IS_HOST0" = true ]; then
  log "web"
  sudo rsync -a --delete "$D/explorer/" "$WWW/explorer/"
  sudo rsync -a --delete "$D/site/" "$WWW/site/"
  sudo rsync -a --delete "$D/site/faucet/" "$WWW/faucet/"
  sudo chown -R root:root "$WWW"; sudo find "$WWW" -type d -exec chmod 755 {} + ; sudo find "$WWW" -type f -exec chmod 644 {} +

  log "api keys"
  # KEEL_API_KEYS (secret): comma-separated `label:token` pairs the clients
  # hold. Empty = no gate on action submission. The map is root-only.
  python3 - "${KEEL_API_KEYS:-}" <<'PY' | root_write 600 "$ETC/api-keys.map"
import sys
entries = [e for e in sys.argv[1].split(',') if e.strip()]
for e in entries:
    label, token = e.split(':', 1)
    token = token.strip(); label = label.strip()
    if '"' in token or '"' in label or not token:
        raise SystemExit('api key entries must be label:token without quotes')
    print(f'"{token}" "{label}";')
PY
  if [ -n "${KEEL_API_KEYS:-}" ]; then
    # sha256 the presented bearer in nginx: the key map holds digests only.
    printf 'if ($keel_key_label = "") { return 401 "{\"error\":\"an API key is required to submit actions\",\"code\":\"API_KEY_REQUIRED\"}"; }\nadd_header X-Keel-Client $keel_key_label always;\n' | root_write 644 "$ETC/api-keys.conf"
    printf 'map $keel_bearer $keel_key_label {\n    default "";\n    include /etc/keelchain/api-keys.map;\n}\n' | root_write 644 /etc/nginx/conf.d/keel-api-keys.conf
  else
    : | root_write 644 "$ETC/api-keys.conf"
    printf 'map $keel_bearer $keel_key_label { default ""; }\n' | root_write 644 /etc/nginx/conf.d/keel-api-keys.conf
  fi
  unset KEEL_API_KEYS

  log "nginx"
  if sudo test -f /etc/ssl/keelchain/origin.pem && sudo test -f /etc/ssl/keelchain/origin.key; then
    sudo install -m 644 "$D/infra/testnet/nginx-keelchain.conf" /etc/nginx/sites-available/keelchain
    sudo ln -sf /etc/nginx/sites-available/keelchain /etc/nginx/sites-enabled/keelchain
    [ -f /etc/nginx/conf.d/keel-upgrade-map.conf ] || printf "map \$http_upgrade \$connection_upgrade { default upgrade; '' close; }\n" | root_write 644 /etc/nginx/conf.d/keel-upgrade-map.conf
    sudo nginx -t && sudo systemctl reload nginx
  else
    echo "no origin certificate in /etc/ssl/keelchain: nginx config not installed (see README)"
  fi
fi

log "health"
sleep 5
curl -s $RPC/v1/status; echo
[ "$IS_HOST0" = true ] && { curl -s http://127.0.0.1:$IDX_PORT/v1/health; echo; }
# shellcheck disable=SC2086
systemctl is-active $UNITS | tr '\n' ' '; echo
rm -rf "$D"
