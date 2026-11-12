#!/usr/bin/env bash
# Preflight for the apt-scout delivery team in .pi/agents/.
# Checks everything the five agents need before they can run on this machine, prints one
# PASS/FAIL/WARN line per check with a one-line fix, and exits 1 if a required check fails.
# Usage: bash scripts/agent-doctor.sh
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
root=$(cd -- "$script_dir/.." && pwd -P)
agents_dir=$root/.pi/agents
# Pi resolves its user-level directory from PI_CODING_AGENT_DIR, defaulting to ~/.pi/agent.
agent_dir=${PI_CODING_AGENT_DIR:-${HOME:-}/.pi/agent}
trust_file=$agent_dir/trust.json

required_agents="apt-scout-lead apt-scout-architect apt-scout-coder apt-scout-reviewer apt-scout-security"

failures=0
warnings=0
pass() { printf 'PASS  %s\n' "$1"; }
fail() { printf 'FAIL  %s\n' "$1"; printf '      fix: %s\n' "$2"; failures=$((failures + 1)); }
warn() { printf 'WARN  %s\n' "$1"; printf '      fix: %s\n' "$2"; warnings=$((warnings + 1)); }

# Canonical form of a path Pi would store in trust.json: realpath when the path exists.
canon() { ( cd -- "$1" 2>/dev/null && pwd -P ) || printf '%s\n' "$1"; }

# Value of the `name:` key in a file's YAML frontmatter, empty when absent.
frontmatter_name() {
  awk '
    NR == 1 { if ($0 !~ /^---[[:space:]]*$/) exit; next }
    /^---[[:space:]]*$/ { exit }
    /^name:/ {
      sub(/^name:[[:space:]]*/, "")
      gsub(/\r$/, "")
      sub(/[[:space:]]+$/, "")
      print
      exit
    }
  ' "$1"
}

# Skill names the agents ask for, read out of their "Skills you apply" sections.
referenced_skills() {
  awk '
    /^##[[:space:]]+Skills you apply/ { in_section = 1; next }
    in_section && /^##[[:space:]]/ { in_section = 0 }
    in_section { print }
  ' "$@" | grep -o '`[a-z][a-z0-9-]*`' | tr -d '`' | sort -u
}

# --- 1. This checkout, and the agent files it is supposed to carry ----------------------

toplevel=$(git -C "$root" rev-parse --show-toplevel 2>/dev/null || true)
if [ -n "$toplevel" ]; then
  toplevel=$(canon "$toplevel")
fi
if [ "$toplevel" = "$root" ]; then
  pass "[1a] repo root is this checkout ($root)"
else
  fail "[1a] repo root is not this checkout (script: $root, git toplevel: ${toplevel:-none})" \
    "keep scripts/agent-doctor.sh inside the clone and run it as bash <clone>/scripts/agent-doctor.sh"
fi

missing=""
extra=""
for agent in $required_agents; do
  [ -f "$agents_dir/$agent.md" ] || missing="$missing $agent"
done
# AGENTS.md is this folder's local contract document, not an agent; every other stray .md
# file is a problem because Pi discovers project agents from .pi/agents/**/*.md.
for file in "$agents_dir"/*.md; do
  [ -f "$file" ] || continue
  name=$(basename "$file" .md)
  case " $required_agents " in
    *" $name "*) ;;
    *) case "$name" in AGENTS | README) ;; *) extra="$extra $name" ;; esac ;;
  esac
done
if [ -z "$missing" ] && [ -z "$extra" ]; then
  pass "[1b] .pi/agents holds exactly the five agents"
else
  [ -z "$missing" ] || fail "[1b] agent file missing:$missing" \
    "restore the file from git: git -C $root checkout -- .pi/agents/"
  [ -z "$extra" ] || fail "[1b] unexpected agent file(s):$extra" \
    "remove them; Pi loads every .pi/agents/**/*.md as an agent and a stray name shadows delegation"
fi

# --- 2. frontmatter name matches the filename -------------------------------------------

names_ok=0
names_bad=0
for agent in $required_agents; do
  file=$agents_dir/$agent.md
  [ -f "$file" ] || continue
  name=$(frontmatter_name "$file")
  if [ "$name" = "$agent" ]; then
    names_ok=$((names_ok + 1))
  else
    names_bad=$((names_bad + 1))
    fail "[2] $agent.md frontmatter name is '${name:-<missing>}', not '$agent'" \
      "set 'name: $agent' in the frontmatter of .pi/agents/$agent.md; a mismatch breaks delegation by name"
  fi
done
if [ "$names_bad" -eq 0 ] && [ "$names_ok" -gt 0 ]; then
  pass "[2] every agent file declares a frontmatter name equal to its filename ($names_ok files)"
fi

# --- 3. Pi and the subagents extension ---------------------------------------------------

if command -v pi >/dev/null 2>&1; then
  pass "[3a] pi on PATH ($(command -v pi))"
else
  fail "[3a] pi not found on PATH" \
    "install Pi and put its binary on PATH; 'pi --version' must work in a new shell"
fi

if [ -d "$agent_dir/npm/node_modules/pi-subagents" ]; then
  pass "[3b] pi-subagents extension installed ($agent_dir/npm/node_modules/pi-subagents)"
else
  fail "[3b] pi-subagents extension not installed at $agent_dir/npm/node_modules/pi-subagents" \
    "pi install npm:pi-subagents; without it there is no subagent tool and the team cannot run"
fi

# --- 4. Project trust: without it Pi ignores .pi/agents entirely -------------------------

# Pi trusts a directory when the nearest entry walking up from it is true, so a trusted
# ancestor counts exactly as Pi would count it during launch.
trust_covered=""
trust_denied=""
dir=$root
while :; do
  entry=$(sed -E -n 's/^[[:space:]]*"(.*)"[[:space:]]*:[[:space:]]*(true|false).*$/\1|\2/p' "$trust_file" 2>/dev/null |
    while IFS='|' read -r key decision; do
      [ "$(canon "$key")" = "$dir" ] && printf '%s\n' "$decision"
    done | head -n 1 || true)
  case "$entry" in
    true)
      trust_covered=$dir
      break
      ;;
    false)
      trust_denied=$dir
      break
      ;;
  esac
  parent=$(dirname "$dir")
  [ "$parent" = "$dir" ] && break
  dir=$parent
done
if [ -n "$trust_covered" ] && [ "$trust_covered" = "$root" ]; then
  pass "[4] project trusted in $trust_file ($root)"
elif [ -n "$trust_covered" ]; then
  pass "[4] project trusted through parent folder $trust_covered; launching Pi in $root itself is covered by it"
elif [ -n "$trust_denied" ]; then
  fail "[4] project marked untrusted in $trust_file ($trust_denied)" \
    "launch pi in $trust_denied, run /trust and allow it; while it is refused Pi ignores .pi/agents and the team is invisible"
else
  fail "[4] $root is not in $trust_file" \
    "launch pi in $root, answer the trust prompt or run /trust; without trust Pi ignores .pi/agents and the team silently disappears"
fi

# --- 5. Rust toolchain -------------------------------------------------------------------

rust_missing=""
for tool in cargo rustfmt; do
  command -v "$tool" >/dev/null 2>&1 || rust_missing="$rust_missing $tool"
done
if command -v cargo >/dev/null 2>&1; then
  cargo clippy --version >/dev/null 2>&1 || rust_missing="$rust_missing clippy"
fi
# Run from the checkout so a rustup toolchain override, if any, is what answers.
cargo_version=$( cd "$root" && cargo --version 2>/dev/null || true )
if [ -n "$rust_missing" ]; then
  fail "[5] Rust toolchain incomplete:$rust_missing" \
    "rustup toolchain install stable --component rustfmt --component clippy"
elif [ ! -f "$root/rust-toolchain.toml" ]; then
  fail "[5] $root/rust-toolchain.toml is missing" \
    "restore it from git: git -C $root checkout -- rust-toolchain.toml"
elif [ -z "$cargo_version" ]; then
  fail "[5] cargo --version fails in $root" \
    "fix the toolchain rust-toolchain.toml selects: rustup show && rustup toolchain install stable"
else
  pass "[5] cargo, rustfmt, clippy available; cargo --version runs in the checkout ($cargo_version)"
fi

# --- 6. Docker daemon (the live suite in scripts/docker-test.sh needs it) ---------------

if docker info >/dev/null 2>&1; then
  pass "[6] Docker daemon reachable"
else
  fail "[6] Docker daemon not reachable" \
    "start Docker (Docker Desktop, or 'sudo systemctl start docker'); only scripts/docker-test.sh needs it, but that is a pre-merge gate"
fi

# --- 7. GitHub CLI auth and repo access --------------------------------------------------

if gh auth status >/dev/null 2>&1; then
  pass "[7a] gh authenticated"
else
  fail "[7a] gh is not authenticated" \
    "gh auth login; the loop opens and merges PRs with gh"
fi

if ( cd "$root" && gh repo view --json nameWithOwner -q .nameWithOwner >/dev/null 2>&1 ); then
  pass "[7b] gh repo view works from this checkout"
else
  fail "[7b] gh repo view fails from $root" \
    "check the origin remote and gh auth status; the loop needs a GitHub repository with push access"
fi

# --- 8. Skills the agent files reference -------------------------------------------------

skills=$(referenced_skills "$agents_dir"/*.md || true)
if [ -z "$skills" ]; then
  fail "[8] no skill references found in .pi/agents/*.md" \
    "restore the 'Skills you apply' sections from git: git -C $root checkout -- .pi/agents/"
else
  missing_skills=0
  for skill in $skills; do
    if compgen -G "$agent_dir/skills/$skill/SKILL.md" >/dev/null ||
      compgen -G "$agent_dir/skills/*/$skill/SKILL.md" >/dev/null ||
      compgen -G "$agent_dir/npm/node_modules/*/skills/$skill/SKILL.md" >/dev/null; then
      continue
    fi
    missing_skills=$((missing_skills + 1))
    fail "[8] skill not installed: $skill (belongs in $agent_dir/skills/$skill/SKILL.md)" \
      "install it to $agent_dir/skills/$skill/SKILL.md; the agents that cite it lose that discipline"
  done
  if [ "$missing_skills" -eq 0 ]; then
    pass "[8] all referenced skills installed ($(printf '%s' "$skills" | tr '\n' ' ' | sed 's/ $//'))"
  fi
fi

# --- 9. Optional conveniences: reported, never fatal ------------------------------------

if command -v graft >/dev/null 2>&1; then
  pass "[9a] graft on PATH (the lead reads structure from the graph)"
else
  warn "[9a] graft not on PATH" "install graft; the lead falls back to rg and git log for structure"
fi

if command -v cargo-deny >/dev/null 2>&1; then
  pass "[9b] cargo-deny on PATH (deny.toml gate)"
else
  warn "[9b] cargo-deny not on PATH" \
    "cargo install cargo-deny; the Cargo.toml license and advisory gate runs it in CI"
fi

if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  pass "[9c] CARGO_TARGET_DIR set ($CARGO_TARGET_DIR)"
else
  warn "[9c] CARGO_TARGET_DIR is unset" \
    "export CARGO_TARGET_DIR=/tmp/apt-scout-target; worktrees then reuse incremental builds"
fi

# --- Summary ----------------------------------------------------------------------------

if [ "$failures" -gt 0 ]; then
  printf '\nagent-doctor: %d required check(s) failed, %d warning(s). Fix the FAIL lines and run again.\n' \
    "$failures" "$warnings"
  exit 1
fi
printf '\nagent-doctor: all required checks passed, %d warning(s). Launch apt-scout-lead with the task.\n' \
  "$warnings"
