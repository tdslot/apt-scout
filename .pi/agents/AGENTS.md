# .pi/agents

## Purpose

Local contracts for the `apt-scout` delivery team: `apt-scout-lead`, `apt-scout-architect`, `apt-scout-coder`, `apt-scout-reviewer`, `apt-scout-security`. Each file is that agent's own instruction set; this doc covers only what they share.

## Ownership

- This folder is off-limits to every delivery agent, together with the root `AGENTS.md`, `.github/**`, `deny.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `.cargo/**`, and the dependency set in `Cargo.toml`. A change to any of them goes to the human as a `chore:` PR; the loop stops instead.
- Delegation must pass `agentScope: project` (or `both`). The runtime default is already `both`, which loads the user-level agents and this folder together; `project` loads this folder alone, so a same-named user-level agent cannot shadow the team.
- The sibling `.pi/README.md` documents this same boundary from the operator's side: how the loop is launched, what a machine needs, and what `scripts/agent-doctor.sh` checks and does not check. It states no rules; this file does.

## Local Contracts

- The model is a machine-level rule, not a project rule: the global `~/.pi/agent/AGENTS.md` names the model this computer uses and requires it to be passed with `model:` on every `subagent(...)` call. Do not pin `model:` in these agent files — children inherit the caller's model and the project stays portable.
- `apt-scout-reviewer` and `apt-scout-security` are read-only by convention: no file writes, commits, pushes, merges, or branch and worktree mutation. The lead verifies it — clean worktree, `HEAD` unchanged since the gates — because the runtime does not enforce it.
- One writer per worktree: `apt-scout-coder` is the only agent that writes `src/`, `tests/`, `scripts/`, `Cargo.*`.

## Child DOX Index

None.
