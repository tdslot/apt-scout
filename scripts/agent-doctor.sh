#!/usr/bin/env bash
# Preflight for the apt-scout delivery team in .pi/agents/.
# Checks everything the five agents need before they can run on this machine, prints one
# PASS/FAIL/WARN line per check with a one-line fix, and exits 1 if a required check fails.
# Project trust is a warning when the entry is merely missing, and a failure when a recorded
# entry says `false`: Pi loads a project's configuration without any trust decision unless
# the project carries a trust-requiring resource, but it still honours a recorded decision
# (see section 4).
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
# A leading UTF-8 BOM is skipped and a quoted value is unquoted, because both are valid
# frontmatter that a hand-edited agent file ends up with. LC_ALL=C keeps the BOM pattern
# three raw bytes instead of one character.
frontmatter_name() {
  LC_ALL=C awk '
    BEGIN { bom = sprintf("%c%c%c", 239, 187, 191) }
    NR == 1 {
      if (index($0, bom) == 1) $0 = substr($0, 4)
      if ($0 !~ /^---[[:space:]]*$/) exit
      next
    }
    /^---[[:space:]]*$/ { exit }
    /^name:/ {
      sub(/^name:[[:space:]]*/, "")
      gsub(/\r$/, "")
      sub(/[[:space:]]+$/, "")
      if (length($0) >= 2) {
        q = substr($0, 1, 1)
        if ((q == "\"" || q == sprintf("%c", 39)) && substr($0, length($0), 1) == q)
          $0 = substr($0, 2, length($0) - 2)
      }
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
  pi_version=$(pi --version 2>/dev/null | head -n 1 || true)
  pass "[3a] pi on PATH ($(command -v pi), version ${pi_version:-unreported})"
else
  fail "[3a] pi not found on PATH" \
    "install Pi and put its binary on PATH; 'pi --version' must work in a new shell"
fi

if [ -d "$agent_dir/npm/node_modules/pi-subagents" ]; then
  pass "[3b] pi-subagents extension installed ($agent_dir/npm/node_modules/pi-subagents)"
elif [ -d "$root/.pi/npm/node_modules/pi-subagents" ]; then
  pass "[3b] pi-subagents extension installed for this project only ($root/.pi/npm/node_modules/pi-subagents)"
else
  fail "[3b] pi-subagents extension not installed (looked in $agent_dir/npm/node_modules and $root/.pi/npm/node_modules)" \
    "pi install npm:pi-subagents; without it there is no subagent tool and the team cannot run"
fi

# The agent files defer two things to user-level config that the repository cannot carry.
# Both are reported, never fatal: the team still runs, with that discipline missing.
machine_agents=$agent_dir/AGENTS.md
if [ -f "$machine_agents" ]; then
  pass "[3c] $machine_agents present (carries the machine-level model rule)"
else
  warn "[3c] $machine_agents is missing" \
    "the agent files pin no model on purpose and defer that rule to this file; without it children just inherit the caller's model and nothing states the machine's choice"
fi

plan_template=$agent_dir/prompts/task-pipeline-orchestrator.md
if [ -f "$plan_template" ]; then
  pass "[3d] $plan_template present (the lead's task-pipeline template)"
else
  warn "[3d] $plan_template is missing" \
    "apt-scout-lead cites this global template by name; restore it so the lead plans from the same pipeline on every machine"
fi

# --- 4. Project trust: a recorded decision is better, not required for this checkout -----

# Pi loads a project's configuration without any trust decision unless the project carries a
# trust-requiring resource (`settings.json`, `mcp.json`, `extensions`, `skills`, `prompts`,
# `themes`, `SYSTEM.md`, `APPEND_SYSTEM.md` under the project config folder, or an
# `.agents/skills` folder above the launch directory other than the user's own). This
# checkout carries agent files and docs only, so Pi shows no prompt here and needs no entry.
# It still reads a recorded decision, though: `project-trusted = trust-store.get(cwd) ?? true`
# in main.js, so a recorded `false` disables the project's agents and settings anyway, while
# a missing entry is only worth a WARN. Hence: missing -> WARN, `false` -> FAIL.

# Nearest decision Pi would use for a directory, printed as "<dir>|<decision>" when found.
# Pi resolves the launch path to its real path and compares that to the keys exactly as
# stored, and it stores the real path itself, so keys are compared as written here.
nearest_trust_entry() {
  local dir=$1 decision parent
  while :; do
    decision=$(sed -E -n 's/^[[:space:]]*"(.*)"[[:space:]]*:[[:space:]]*(true|false).*$/\1|\2/p' "$trust_file" 2>/dev/null |
      while IFS='|' read -r key value; do
        [ "$key" = "$dir" ] && printf '%s\n' "$value"
      done | head -n 1 || true)
    if [ -n "$decision" ]; then
      printf '%s|%s\n' "$dir" "$decision"
      return 0
    fi
    parent=$(dirname "$dir")
    [ "$parent" = "$dir" ] && return 0
    dir=$parent
  done
}

# Trust-requiring resources Pi would find from a directory; empty when there are none.
trust_requiring_resources() {
  local dir=$1 entry found="" parent user_skills
  for entry in settings.json mcp.json extensions skills prompts themes SYSTEM.md APPEND_SYSTEM.md; do
    if [ -e "$dir/.pi/$entry" ]; then
      found="${found:+$found }.pi/$entry"
    fi
  done
  user_skills=$(canon "${HOME:-}/.agents/skills")
  dir=$(canon "$dir")
  while :; do
    if [ -d "$dir/.agents/skills" ] && [ "$(canon "$dir/.agents/skills")" != "$user_skills" ]; then
      found="${found:+$found }$dir/.agents/skills"
      break
    fi
    parent=$(dirname "$dir")
    [ "$parent" = "$dir" ] && break
    dir=$parent
  done
  printf '%s\n' "$found"
}

report_trust() { # label dir
  local label=$1 dir=$2 entry covered decision resources
  entry=$(nearest_trust_entry "$dir")
  covered=${entry%%|*}
  decision=${entry#*|}
  resources=$(trust_requiring_resources "$dir")
  if [ "$decision" = true ]; then
    if [ -n "$resources" ]; then
      pass "[$label] project trusted in $trust_file ($covered); this checkout needs that decision ($resources)"
    else
      pass "[$label] trust recorded for $covered in $trust_file; Pi needs no decision here and loads project configuration without one too"
    fi
  elif [ "$decision" = false ]; then
    fail "[$label] $covered is marked untrusted in $trust_file, so Pi loads no project agents or project settings from $dir (agentScope: project refuses outright)" \
      "launch pi in $covered, run /trust and allow it, or delete that entry: Pi honours it whether or not the checkout carries a trust-requiring resource"
  elif [ -n "$resources" ]; then
    warn "[$label] no trust decision covers $dir and this checkout needs one ($resources); Pi asks at launch and answers no when the prompt cannot be shown (pi -p)" \
      "launch pi in $dir and allow the prompt, or run /trust; the answer is saved to $trust_file and the prompt stops appearing"
  else
    warn "[$label] no trust entry for $dir in $trust_file; Pi needs none here, because this checkout carries no trust-requiring resource, and loads project configuration without a prompt" \
      "run /trust in pi, or add \"$dir\": true to $trust_file: nothing changes today, and it keeps the team loading if a trust-requiring resource (.pi/settings.json, .agents/skills) is added or a false entry is recorded"
  fi
}

# What Pi would do for a directory, so a second directory is only reported when it differs.
trust_state() { # dir
  local entry
  entry=$(nearest_trust_entry "$1")
  printf '%s|%s\n' "${entry#*|}" "$(trust_requiring_resources "$1")"
}

root_trust=$(trust_state "$root")
report_trust "4a" "$root"
# A Pi launched deeper inside the clone resolves the same decisions for almost every path, so
# the launch directory is reported only when trusting it would actually turn out differently.
launch_dir=$(canon "${PWD:-$root}")
case "$launch_dir" in
  "$root"/*)
    if [ "$(trust_state "$launch_dir")" != "$root_trust" ]; then
      report_trust "4b" "$launch_dir"
    fi
    ;;
esac

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

# --- 7. Git and GitHub access, and the commit identity the loop commits under ------------

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
    "check the origin remote and gh auth status; the loop needs a GitHub repository it can push to"
fi

# Commits carry the identity git resolves here; the coder and the lead commit on every task.
# `git var` is what a commit actually consults: config, environment and system fallbacks.
if commit_ident=$(git -C "$root" var GIT_AUTHOR_IDENT 2>/dev/null) && [ -n "$commit_ident" ]; then
  pass "[7c] git resolves a commit identity in this checkout"
else
  fail "[7c] git cannot resolve a commit identity in $root" \
    "git config user.name '<name>' and git config user.email '<email>'; without them every commit the loop makes is rejected or attributed wrong"
fi

# gh's own permission on the repository. It is not the credential the git remote pushes
# with, so a PASS here only rules out a read-only token, and the note says exactly that.
push_perm=$( cd "$root" && gh repo view --json viewerPermission -q .viewerPermission 2>/dev/null || true )
case "$push_perm" in
  ADMIN | MAINTAIN | WRITE)
    pass "[7d] gh reports $push_perm on the repository (its token can open and merge PRs; the git remote's own push credentials are not checked here)"
    ;;
  READ | TRIAGE)
    fail "[7d] gh reports $push_perm on the repository" \
      "re-auth with an account or token that can write: gh auth login; the loop cannot push a branch or merge a PR with $push_perm"
    ;;
  *)
    warn "[7d] gh did not report a repository permission (got '${push_perm:-nothing}')" \
      "check gh auth status and the origin remote by hand; this proves nothing either way"
    ;;
esac

# --- 8. Skills the agent files reference -------------------------------------------------

# Every place Pi loads skills from: the user's agent dir, its npm packages, and the Agent
# Skills folders Pi also supports (docs/skills.md), one level of grouping allowed in each.
skill_installed() { # skill
  local skill=$1
  compgen -G "$agent_dir/skills/$skill/SKILL.md" >/dev/null && return 0
  compgen -G "$agent_dir/skills/*/$skill/SKILL.md" >/dev/null && return 0
  compgen -G "$agent_dir/npm/node_modules/*/skills/$skill/SKILL.md" >/dev/null && return 0
  compgen -G "$agent_dir/npm/node_modules/*/skills/*/$skill/SKILL.md" >/dev/null && return 0
  compgen -G "$root/.pi/npm/node_modules/*/skills/$skill/SKILL.md" >/dev/null && return 0
  compgen -G "$root/.pi/npm/node_modules/*/skills/*/$skill/SKILL.md" >/dev/null && return 0
  if [ -n "${HOME:-}" ]; then
    compgen -G "$HOME/.agents/skills/$skill/SKILL.md" >/dev/null && return 0
    compgen -G "$HOME/.agents/skills/*/$skill/SKILL.md" >/dev/null && return 0
  fi
  compgen -G "$root/.agents/skills/$skill/SKILL.md" >/dev/null && return 0
  compgen -G "$root/.agents/skills/*/$skill/SKILL.md" >/dev/null && return 0
  return 1
}

skills=$(referenced_skills "$agents_dir"/*.md || true)
if [ -z "$skills" ]; then
  fail "[8] no skill references found in .pi/agents/*.md" \
    "restore the 'Skills you apply' sections from git: git -C $root checkout -- .pi/agents/"
else
  missing_skills=0
  for skill in $skills; do
    skill_installed "$skill" && continue
    missing_skills=$((missing_skills + 1))
    fail "[8] skill not installed: $skill (belongs in $agent_dir/skills/$skill/SKILL.md)" \
      "install it to $agent_dir/skills/$skill/SKILL.md; Pi also finds it in ~/.agents/skills or .agents/skills. The agents that cite it lose that discipline"
  done
  if [ "$missing_skills" -eq 0 ]; then
    pass "[8] all referenced skills installed ($(printf '%s' "$skills" | tr '\n' ' ' | sed 's/ $//'))"
  fi
fi

# --- 9. Optional conveniences: reported, never fatal ------------------------------------

if command -v graft >/dev/null 2>&1; then
  # The lead reads structure from the primary checkout's graph (`apt-scout-lead.md`), so a
  # task worktree legitimately has none of its own and that is not a problem to report.
  primary_worktree=$(git -C "$root" worktree list --porcelain 2>/dev/null |
    awk '/^worktree /{sub(/^worktree /, ""); print; exit}' || true)
  if [ -f "$root/graft/INDEX.md" ]; then
    pass "[9a] graft on PATH and this checkout has a graph ($root/graft/INDEX.md)"
  elif [ -n "$primary_worktree" ] && [ "$primary_worktree" != "$root" ] && [ -f "$primary_worktree/graft/INDEX.md" ]; then
    pass "[9a] graft on PATH; this worktree has no graph of its own and reads the primary checkout's ($primary_worktree/graft/INDEX.md)"
  else
    warn "[9a] graft on PATH but no checkout has a graph" \
      "run 'graft build' in $root; the lead reads structure from graft/INDEX.md and falls back to rg and git log without it"
  fi
else
  warn "[9a] graft not on PATH" "install graft; the lead falls back to rg and git log for structure"
fi

# The gate is 'cargo deny check'. cargo finds the subcommand through its own bin directory
# ($CARGO_HOME/bin) even when that directory is not on PATH, so ask cargo first: testing
# 'command -v cargo-deny' alone reported the gate missing on a machine that runs it.
deny_version=$(cargo deny --version 2>/dev/null || true)
if [ -z "$deny_version" ] && command -v cargo-deny >/dev/null 2>&1; then
  deny_version=$(cargo-deny --version 2>/dev/null || true)
fi
if [ -n "$deny_version" ]; then
  if command -v cargo-deny >/dev/null 2>&1; then
    pass "[9b] $deny_version available (the Cargo.toml license and advisory gate can run here)"
  else
    pass "[9b] $deny_version available; cargo finds it through \$CARGO_HOME/bin, which is not on PATH"
  fi
else
  # WARN, not FAIL: CI runs 'cargo deny check' as a required status check, so a local
  # absence leaves the gate verified by CI. Docker is a FAIL above because
  # scripts/docker-test.sh never runs in CI, so nothing else can verify it.
  warn "[9b] cargo-deny not available ('cargo deny --version' and 'command -v cargo-deny' both fail)" \
    "cargo install cargo-deny; the Cargo.toml license and advisory gate runs it in CI as a required check, so only a local run is missing"
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
