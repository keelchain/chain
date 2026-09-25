#!/usr/bin/env bash
# Full local check: clippy deny-lists, every test, TS SDK vectors.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.local/bin:$PATH"   # m4 for the GMP build behind cggmp21
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
if command -v npm >/dev/null && [ -d ../sdk/ts/node_modules ]; then
  (cd ../sdk/ts && npm test)
fi
echo "check: ok"
