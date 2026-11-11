#!/usr/bin/env bash
# Print a CHANGELOG section for <version> built from Conventional Commits since the last tag.
# Usage: changelog.sh <version>
set -euo pipefail
version=${1:?usage: changelog.sh <version>}
prev=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
log=$(git log --no-merges --pretty='%s' ${prev:+"$prev..HEAD"})

body=""
section() { # <title> <subject regex>
  local items
  items=$(grep -E "$2" <<<"$log" | sed -E 's/^[a-z]+(\([^)]*\))?!?: //; s/^/- /' || true)
  if [ -n "$items" ]; then body+=$'\n'"### $1"$'\n\n'"$items"$'\n'; fi
}
section "Breaking changes" '^[a-z]+(\([^)]*\))?!:'
section "Added" '^feat(\([^)]*\))?:'
section "Fixed" '^fix(\([^)]*\))?:'
section "Changed" '^(perf|refactor)(\([^)]*\))?:'
section "Documentation" '^docs(\([^)]*\))?:'
[ -n "$body" ] || body=$'\nNo user-facing changes.\n'

printf '## %s - %s\n%s' "$version" "$(date -u +%F)" "$body"
