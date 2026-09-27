#!/usr/bin/env bash
# Renders the markdown docs to HTML pages under $OUT (default site/docs in a
# build directory) using the site's template, so every API link on
# keelchain.com lands on a documentation page rather than on raw JSON.
#
#   infra/dev/render-docs.sh [OUT]
#
# Needs pandoc. Pages are written as <OUT>/<slug>/index.html so URLs are
# keelchain.com/docs/<slug>/. The list below is the docs index.
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT=${1:-out/site/docs}
TEMPLATE=site/docs/template.html
mkdir -p "$OUT"

# slug | source | title | one-line description (for the index)
PAGES=(
  "models|docs/models.md|Two ways to run an exchange on Keel|Custodial clients with their own wallets, P2P clients on Keel Wallet: where funds sit, who signs, what it costs."
  "clients|docs/clients.md|Building a client|The integration contract: accounts and roles, the per-network switch and readiness, idempotency and reconciliation, streams, signing, pricing."
  "api|docs/explorer-api.md|Read API|The indexer's read API: blocks, accounts, markets, offers, trades, vaults, governance, search and the WebSocket feed."
  "rpc|chain/crates/keel-rpc/API.md|Node RPC|The node's HTTP API: status, accounts, submitting signed actions, receipts, order book, vaults, governance, readiness, state sync."
  "ws|chain/crates/keel-rpc/API-WS.md|WebSocket subscriptions|Filtered, sequenced subscriptions on the node and the indexer: accounts, pairs, deposits, order books, gaps and replay."
  "deposits|docs/deposits.md|Deposits and withdrawals|Funding an account with Bitcoin or Tron on the testnet, confirmation depths, and withdrawing."
  "wallet|docs/wallet.md|Keel Wallet|The wallet's provider API, key model and what a client site needs to connect."
  "connect-keel-wallet|docs/connect-keel-wallet.md|Connect Keel Wallet|Adding a Connect button to a site or app: the provider, login challenges, signing, session keys, mobile."
  "explorer-api|docs/explorer-api.md|Explorer API|Alias of the read API page."
  "how-it-works|docs/how-it-works.md|How the chain works|Consensus, issuance, governance, fees, wallets, custody, markets, P2P trading and the resilience scenarios."
  "tokenomics|docs/tokenomics.md|Tokenomics|KEEL supply, allocation and the fee model."
  "testnet|docs/testnet.md|The public testnet|What runs where, how to fund an account and how clients are onboarded."
  "lightning|docs/lightning.md|Lightning|Bitcoin Lightning deposits and payouts through observer pools."
  "whitepaper|docs/whitepaper.md|Whitepaper|Why a chain, and why this shape."
)

render() {
  local slug=$1 src=$2 title=$3
  mkdir -p "$OUT/$slug"
  # Markdown docs link to each other as `name.md`; rewrite those to site paths.
  sed -E 's~\]\(([a-z0-9-]+)\.md(#[^)]*)?\)~](../\1/\2)~g' "$src" \
    | pandoc --from gfm --to html5 --standalone --template "$TEMPLATE" \
        --toc --toc-depth=2 \
        --metadata title="$title" --metadata root="../../" --metadata source="$src" \
        --output "$OUT/$slug/index.html"
}

{
  echo "# Documentation"
  echo
  echo "Everything here is rendered from the markdown in the public repository, so the"
  echo "page you read is the file the code is built from."
  echo
  for entry in "${PAGES[@]}"; do
    IFS='|' read -r slug src title desc <<<"$entry"
    [ "$slug" = explorer-api ] && continue
    echo "- [$title]($slug/): $desc"
  done
  echo
  echo "## Live endpoints"
  echo
  echo "- Read API: \`https://testnet.keelchain.com/api/v1/\` (health at \`/api/v1/health\`)"
  echo "- Node RPC: \`https://testnet.keelchain.com/rpc/v1/\` (status at \`/rpc/v1/status\`)"
  echo "- Explorer: <https://testnet.keelchain.com/>"
  echo "- SDK: \`npm install @keelchain/sdk\`"
} | pandoc --from gfm --to html5 --standalone --template "$TEMPLATE" \
      --metadata title="Documentation" --metadata root="../" --metadata source="docs/" \
      --output "$OUT/index.html"

for entry in "${PAGES[@]}"; do
  IFS='|' read -r slug src title desc <<<"$entry"
  render "$slug" "$src" "$title"
done
echo "rendered $((${#PAGES[@]} + 1)) pages into $OUT"
