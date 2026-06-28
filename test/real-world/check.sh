#!/usr/bin/env bash
# Parse every fetched real-world fixture and fail if any file produces more
# ERROR nodes than allowed in sources.tsv. Run fetch.sh first.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
dest="$here/fixtures"

ts() {
  if command -v tree-sitter >/dev/null 2>&1; then tree-sitter "$@";
  else npx --no-install tree-sitter "$@"; fi
}

if [ ! -d "$dest" ] || [ -z "$(ls -A "$dest" 2>/dev/null)" ]; then
  echo "no fixtures found; run $here/fetch.sh first" >&2
  exit 2
fi

cd "$root"
status=0
total=0
while IFS=$'\t' read -r max name url; do
  case "$max" in '#'*|'') continue ;; esac
  file="$dest/$name"
  [ -f "$file" ] || { printf 'SKIP %-28s (not fetched)\n' "$name"; continue; }
  errs=$(ts parse "$file" 2>/dev/null | grep -c 'ERROR' || true)
  total=$((total + errs))
  if [ "$errs" -le "$max" ]; then
    printf 'PASS %-28s %s error(s) (<= %s)\n' "$name" "$errs" "$max"
  else
    printf 'FAIL %-28s %s error(s) (> %s)\n' "$name" "$errs" "$max"
    status=1
  fi
done < "$here/sources.tsv"

echo "total ERROR nodes across corpus: $total"
exit $status
