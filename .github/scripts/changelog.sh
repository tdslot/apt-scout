#!/usr/bin/env bash
# Print a CHANGELOG section for <version> built from Conventional Commits since the last tag.
# Usage: changelog.sh <version>
set -euo pipefail
version=${1:?usage: changelog.sh <version>}
# Tags sit on main merge commits, which are not ancestors of develop, so describe cannot find them.
prev=$(git tag -l 'v*' --sort=-v:refname | head -n1)
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
