#!/usr/bin/env bash
# Download the real-world batch-file fixtures listed in sources.tsv into
# ./fixtures/ (gitignored). Safe to re-run; existing files are overwritten.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dest="$here/fixtures"
mkdir -p "$dest"

ok=0
fail=0
while IFS=$'\t' read -r _max name url; do
  case "$_max" in '#'*|'') continue ;; esac
  if curl -fsSL --max-time 30 "$url" -o "$dest/$name"; then
    printf 'OK   %-28s %s lines\n' "$name" "$(wc -l < "$dest/$name")"
    ok=$((ok + 1))
  else
    printf 'FAIL %-28s %s\n' "$name" "$url"
    fail=$((fail + 1))
  fi
done < "$here/sources.tsv"

echo "fetched $ok file(s), $fail failure(s) into $dest"
