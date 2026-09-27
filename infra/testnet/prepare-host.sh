#!/usr/bin/env bash
# One-time preparation of a testnet host, run by me over SSH as a sudoer
# before the first deploy. Everything the deploy does not manage lives here:
# packages, the WireGuard key, firewall rules, and on host 0 the shared
# signet bitcoind, Docker with the indexer's Postgres, and nginx.
#
#   prepare-host.sh <index> <wg_ip> [--host0]
#
# Prints this host's WireGuard public key at the end; it goes into the
# WG_PUBKEYS variable of the GitHub environment (the private key stays in
# /etc/wireguard/keel.key and is also stored as the WG_PRIVATE_KEY_<i>
# secret, read once with `sudo cat`).
set -euo pipefail
N=${1:?host index}; WG_IP=${2:?wireguard address, e.g. 10.90.0.1}; HOST0=false
[ "${3:-}" = "--host0" ] && HOST0=true
[ "$N" = 0 ] && HOST0=true

sudo apt-get update
sudo apt-get install -y wireguard-tools ufw curl python3 rsync age rclone prometheus-node-exporter
# node_exporter: systemd collector on, textfile metrics from keel-probe.sh,
# reachable by the host-0 Prometheus over the mesh only.
sudo install -d -m 755 /var/lib/node_exporter/textfile
printf 'ARGS="--collector.systemd --collector.textfile.directory=/var/lib/node_exporter/textfile --web.listen-address=%s:9100"\n' "$WG_IP" | sudo tee /etc/default/prometheus-node-exporter >/dev/null
sudo systemctl enable prometheus-node-exporter >/dev/null 2>&1 || true
if [ "$HOST0" = true ]; then
  sudo apt-get install -y nginx docker.io
fi

# WireGuard identity (kept; the deploy renders the config around it).
if ! sudo test -f /etc/wireguard/keel.key; then
  sudo install -d -m 700 /etc/wireguard
  umask 077
  wg genkey | sudo tee /etc/wireguard/keel.key >/dev/null
  sudo chmod 600 /etc/wireguard/keel.key
fi
WG_PUB=$(sudo cat /etc/wireguard/keel.key | wg pubkey)

# Firewall: ssh and the p2p port from anywhere; https on host 0; the mesh,
# the signer port and bitcoind only over WireGuard.
sudo ufw --force reset >/dev/null
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow 22/tcp
sudo ufw allow 3000/tcp
sudo ufw allow 51820/udp
sudo ufw allow in on keel to any port 7000 proto tcp
sudo ufw allow in on keel to any port 9100 proto tcp
if [ "$HOST0" = true ]; then
  sudo ufw allow 80,443/tcp
  sudo ufw allow in on keel to any port 38332 proto tcp
fi
sudo ufw --force enable

# Kernel and file limits the validator relies on.
printf 'net.core.rmem_max=8388608\nnet.core.wmem_max=8388608\nvm.swappiness=10\n' | sudo tee /etc/sysctl.d/90-keel.conf >/dev/null
sudo sysctl -q --system

sudo install -d -m 755 /opt/keelchain /var/lib/keelchain /var/www/keelchain
sudo install -d -m 700 /etc/keelchain /var/lib/keelchain/tss

if [ "$HOST0" = true ]; then
  # Indexer database.
  if ! sudo docker ps -a --format '{{.Names}}' | grep -qx keel-pg; then
    PW=$(python3 -c 'import secrets; print(secrets.token_urlsafe(24))')
    sudo docker run -d --name keel-pg --restart unless-stopped -p 127.0.0.1:5434:5432 \
      -e POSTGRES_USER=keel -e POSTGRES_PASSWORD="$PW" -e POSTGRES_DB=keel_indexer_testnet \
      -v keel-pg:/var/lib/postgresql/data --memory 256m postgres:16-alpine >/dev/null
    echo "postgres container keel-pg created (password in its environment; read it with docker inspect when setting INDEXER_DATABASE_URL)"
  fi
  # Shared signet bitcoind: pruned, RPC on loopback and on the mesh address.
  if ! command -v bitcoind >/dev/null; then
    echo "install Bitcoin Core (bitcoind, bitcoin-cli) into /usr/local/bin before running the deploy" >&2
  fi
  sudo install -d -m 750 /var/lib/bitcoin-signet
  if ! sudo test -f /var/lib/bitcoin-signet/bitcoin.conf; then
    RPCPW=$(python3 -c 'import secrets; print(secrets.token_urlsafe(24))')
    printf 'signet=1\nprune=1500\nserver=1\ntxindex=0\n[signet]\nrpcuser=keel\nrpcpassword=%s\nrpcbind=127.0.0.1\nrpcbind=%s\nrpcallowip=127.0.0.1\nrpcallowip=10.90.0.0/24\nrpcport=38332\n' "$RPCPW" "$WG_IP" | sudo tee /var/lib/bitcoin-signet/bitcoin.conf >/dev/null
    sudo chmod 600 /var/lib/bitcoin-signet/bitcoin.conf
    echo "bitcoin.conf written; its rpcpassword goes into the BITCOIN_RPC_PASSWORD secret"
  fi
  if [ ! -f /etc/systemd/system/bitcoind-signet.service ]; then
    printf '[Unit]\nDescription=Bitcoin Core (signet, pruned)\nAfter=network-online.target wg-quick@keel.service\n\n[Service]\nExecStart=/usr/local/bin/bitcoind -datadir=/var/lib/bitcoin-signet -conf=/var/lib/bitcoin-signet/bitcoin.conf\nRestart=always\nRestartSec=10\nMemoryMax=1200M\n\n[Install]\nWantedBy=multi-user.target\n' | sudo tee /etc/systemd/system/bitcoind-signet.service >/dev/null
    sudo systemctl daemon-reload; sudo systemctl enable bitcoind-signet >/dev/null 2>&1 || true
  fi
  echo "host 0 also needs the Cloudflare origin certificate at /etc/ssl/keelchain/origin.{pem,key}"
fi

echo
echo "host $N prepared. WireGuard public key (for WG_PUBKEYS[$N]):"
echo "$WG_PUB"
