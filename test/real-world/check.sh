#!/usr/bin/env bash
# Parse every committed real-world fixture in ./fixtures/ and fail if any file
# produces more ERROR nodes than its budget in sources.tsv.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
dest="$here/fixtures"

if ! command -v tree-sitter >/dev/null 2>&1; then
  echo "tree-sitter CLI not found in PATH" >&2
  exit 1
fi

cd "$root"
status=0
total=0
while IFS=$'\t' read -r max name url; do
  case "$max" in '#'*|'') continue ;; esac
  file="$dest/$name"
  if [ ! -f "$file" ]; then
    printf 'FAIL %-28s (missing fixture)\n' "$name"
    status=1
    continue
  fi
  errs=$(tree-sitter parse "$file" 2>/dev/null | grep -c 'ERROR' || true)
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
