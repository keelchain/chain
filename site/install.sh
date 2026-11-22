#!/bin/sh
# Install the Keel command-line tools (keel, keel-node, keel-indexer,
# keel-observer, keel-tss) on Linux or macOS.
#
#   curl -fsSL https://keelchain.com/install.sh | sh
#
# KEEL_INSTALL_DIR  where the programs go (default: ~/.local/bin)
# KEEL_DOWNLOADS    where the archives are (default: https://keelchain.com/downloads)
set -eu
BASE=${KEEL_DOWNLOADS:-https://keelchain.com/downloads}
DIR=${KEEL_INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)" in
  Linux) OS=linux ;;
  Darwin) OS=macos ;;
  *) echo "Unsupported system $(uname -s). On Windows use install.ps1; see https://keelchain.com/docs/install/" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) echo "Unsupported processor $(uname -m). Build from source: https://keelchain.com/docs/install/" >&2; exit 1 ;;
esac
NAME="keel-$OS-$ARCH"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Downloading $NAME.tar.gz"
curl -fsSL "$BASE/$NAME.tar.gz" -o "$TMP/$NAME.tar.gz"
curl -fsSL "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS"

WANT=$(grep " $NAME.tar.gz\$" "$TMP/SHA256SUMS" | cut -d' ' -f1)
if command -v sha256sum >/dev/null 2>&1; then GOT=$(sha256sum "$TMP/$NAME.tar.gz" | cut -d' ' -f1)
else GOT=$(shasum -a 256 "$TMP/$NAME.tar.gz" | cut -d' ' -f1); fi
if [ -z "$WANT" ] || [ "$WANT" != "$GOT" ]; then
  echo "Checksum mismatch for $NAME.tar.gz: nothing was installed." >&2; exit 1
fi

tar -xzf "$TMP/$NAME.tar.gz" -C "$TMP"
mkdir -p "$DIR"
for f in "$TMP/$NAME"/keel*; do
  install -m 755 "$f" "$DIR/"
done
echo "Installed in $DIR: $(cd "$TMP/$NAME" && ls keel* | tr '\n' ' ')"
"$DIR/keel" --version
case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo "Add $DIR to your PATH, for example:  export PATH=\"$DIR:\$PATH\"" ;;
esac
