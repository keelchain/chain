#!/usr/bin/env bash
# Fails when any word from infra/dev/forbidden-words.txt appears in the tree.
# Client projects are never named in this repository; a client is a client.
set -uo pipefail
cd "$(dirname "$0")/../.."
hits=$(git ls-files -z \
  | grep -zv -e '^infra/dev/forbidden-words.txt$' -e '^infra/dev/check-forbidden-words.sh$' \
  | xargs -0 grep -n -i -E -f infra/dev/forbidden-words.txt 2>/dev/null || true)
if [ -n "$hits" ]; then
  echo "forbidden words found:" >&2
  echo "$hits" >&2
  exit 1
fi
echo "no forbidden words"
