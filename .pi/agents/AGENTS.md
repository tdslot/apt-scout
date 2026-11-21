# .pi/agents

## Purpose

Local contracts for the `apt-scout` delivery team: `apt-scout-lead`, `apt-scout-architect`, `apt-scout-coder`, `apt-scout-reviewer`, `apt-scout-security`. Each file is that agent's own instruction set; this doc covers only what they share. `lessons.md` is the team's record of its own mistakes and the rule that replaces each one.

## Ownership

- This folder is off-limits to every delivery agent, together with the root `AGENTS.md`, `.github/**`, `deny.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `.cargo/**`, and the dependency set in `Cargo.toml`. A change to any of them goes to the human as a `chore:` PR; the loop stops instead.
- Delegation must pass `agentScope: project` (or `both`). The runtime default is already `both`, which loads the user-level agents and this folder together; `project` loads this folder alone, so a same-named user-level agent cannot shadow the team.
- The sibling `.pi/README.md` documents this same boundary from the operator's side: how the loop is launched, what a machine needs, and what `scripts/agent-doctor.sh` checks and does not check. It states no rules; this file does.

## Local Contracts

- The model is a machine-level rule, not a project rule: the global `~/.pi/agent/AGENTS.md` names the model this computer uses and requires it to be passed with `model:` on every `subagent(...)` call. Do not pin `model:` in these agent files — children inherit the caller's model and the project stays portable.
- `apt-scout-reviewer` and `apt-scout-security` are read-only by convention: no file writes, commits, pushes, merges, or branch and worktree mutation. The lead verifies it — clean worktree, `HEAD` unchanged since the gates — because the runtime does not enforce it.
- One writer per worktree: `apt-scout-coder` is the only agent that writes `src/`, `tests/`, `scripts/`, `Cargo.*`.
- `lessons.md` is the one file in this folder every agent may append to, and the only one: a mistake is recorded by whoever made it, in the same turn, while its context is alive. An entry states the failure mode, why the habit behind it is wrong, and the rule that replaces it — never a diary entry and never an apology.
- A rule that outlives the incident is promoted to the nearest `AGENTS.md` that owns the rule; `lessons.md` keeps the incident. Before a pull request, `rg -n `'^- analyzed: no$'` .pi/agents/lessons.md` returns nothing, because an unclosed raw fact is an unexamined mistake.
- The raw fact of every failed tool call is appended to `lessons.md` by the global Pi extension `~/.pi/agent/extensions/mistake-logger.ts` (tool, arguments, error, `analyzed: no`). The extension writes only that fact; the analysis is written by the agent. In a directory without `.pi/agents` it writes to `~/.pi/agent/lessons.md` instead, so a repository never gains the folder by accident.

## Child DOX Index

None.
