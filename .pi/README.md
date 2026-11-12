# `.pi/` — the apt-scout delivery team

Project configuration for the Pi agents that deliver this crate. `.pi/` holds agent files
and this README only, which is the part of the delivery setup Pi loads without any trust
decision; every other requirement is machine state, which `scripts/agent-doctor.sh` checks
where a script can see it and lists where it cannot.

## The five agents

| Agent | Job |
| --- | --- |
| `apt-scout-lead` | Runs the loop: picks the next task, delegates, runs the gates, merges. The only agent that merges. |
| `apt-scout-architect` | Owns `SPEC.md`, `ROADMAP.md`, `GLOSSARY.md`; writes the plan a coder executes. Writes no code. |
| `apt-scout-coder` | Implements one approved plan in one worktree, with a test per acceptance criterion. The only code writer. |
| `apt-scout-reviewer` | Independent review of the diff, runs the Rust gates itself. Read-only. |
| `apt-scout-security` | Trust-boundary audit of the diff. Read-only. |

`oracle` is not a file here: it is builtin, and the lead uses it for the milestone audit
and for the threat model before any privileged task.

## Running the loop

Start Pi in the clone and hand it one task:

```bash
pi
```

```text
subagent({ agent: "apt-scout-lead", task: "Take the next READY task from ROADMAP.md and run the loop until you stop." })
```

That single launch is the whole loop. The lead reads `ROADMAP.md`, picks the first
`READY` task that is not blocked, classifies it, has `apt-scout-architect` settle the
spec and write `plans/<task>-plan.md`, has `apt-scout-coder` implement it in a fresh
worktree, runs the gates itself, gets a review and a security verdict on the exact diff,
opens the PR, merges into `develop` when every check is green, records the evidence in
`ROADMAP.md` and starts the next task. It stops and asks the human at Gate 1 (plan
approval) or Gate 2 (diff before merge) for complex work: anything reaching the
privilege boundary, a trust boundary, network input, product behavior the SPEC does not
settle, or a new dependency.

Project agents resolve under the default `agentScope: "both"`; pass
`options: { agentScope: "project" }` if you want to be explicit and exclude user-level
agents with the same name.

## One task by hand

To run a single step instead of the loop, make the worktree yourself and launch each
agent with the same `subagent(...)` shape:

```bash
git worktree add /tmp/apt-scout-<task> -b <type>/<slug> origin/develop
```

```text
subagent({ agent: "apt-scout-architect", task: "Write plans/<task>-plan.md for <task> in /tmp/apt-scout-<task>", cwd: "/tmp/apt-scout-<task>" })
subagent({ agent: "apt-scout-coder",     task: "Implement plans/<task>-plan.md in /tmp/apt-scout-<task>", cwd: "/tmp/apt-scout-<task>" })
subagent({ agent: "apt-scout-reviewer",  task: "Review origin/develop...HEAD in /tmp/apt-scout-<task>", cwd: "/tmp/apt-scout-<task>" })
subagent({ agent: "apt-scout-security",  task: "Audit origin/develop...HEAD in /tmp/apt-scout-<task>", cwd: "/tmp/apt-scout-<task>" })
```

One writer per worktree: never run the coder and an editor in the same tree. The
reviewers and the security auditor are read-only by convention, and the lead checks
that they left `HEAD` and the worktree untouched.

## What a new machine needs

`bash scripts/agent-doctor.sh` prints one `PASS`/`FAIL`/`WARN` line per item, the fix for
every failure, and exits non-zero while something required is missing. Each item below
carries the label the doctor prints, so its output maps to a paragraph here.

- **`[1a]`, `[1b]` — this clone with its five agent files.** Restore a missing one with
  `git checkout -- .pi/agents/`; a stray `.md` file in `.pi/agents/` is reported too,
  because Pi loads every one of them as an agent and a stray name shadows delegation.
- **`[2]` — each file's frontmatter `name:` equals its filename.** A mismatch breaks
  delegation by name, which is the only way the lead addresses an agent.
- **`[3a]` — `pi` on `PATH`.** The doctor also prints the version it found.
- **`[3b]` — the `pi-subagents` extension.** `pi install npm:pi-subagents`, user-wide
  (`~/.pi/agent/npm/node_modules`) or for this project alone (`.pi/npm/node_modules`); the
  doctor accepts either. Without that extension there is no subagent tool and the lead cannot
  delegate at all.
- **`[3c]`, `[3d]` — the two user-level files the agents defer to.** `~/.pi/agent/AGENTS.md`
  carries the machine-level model rule, and
  `~/.pi/agent/prompts/task-pipeline-orchestrator.md` is the template the lead names when it
  plans. Warnings, not failures: the team runs without them, with that discipline missing.
- **`[4a]`, `[4b]` — a recorded project-trust decision**, for the clone and for the directory
  the doctor is run from when that is a subdirectory of it. A missing entry only warns; a
  recorded `false` fails. See below.
- **`[5]` — Rust toolchain.** `command -v cargo rustfmt` and `cargo clippy --version` must work
  on the stable toolchain `rust-toolchain.toml` pins:
  `rustup toolchain install stable --component rustfmt --component clippy`.
- **`[6]` — Docker daemon reachable** (`docker info`). A hard failure because
  `scripts/docker-test.sh` runs only on this machine, never in CI, so nothing else can run
  that gate: start Docker Desktop, or `sudo systemctl start docker`.
- **`[7a]`, `[7b]` — `gh` authenticated** (`gh auth login`) and `gh repo view` working from
  the clone — the loop opens and merges PRs through it.
- **`[7c]` — a commit identity.** git must resolve an author identity inside the clone
  (`git var GIT_AUTHOR_IDENT`, which reads configuration and environment); every task ends in
  commits, and a missing identity either fails them or attributes them wrong.
- **`[7d]` — the repository permission `gh` reports.** `READ` and `TRIAGE` fail, because that
  token can neither push a branch nor merge. This is `gh`'s credential, not the one
  `git push` uses; see below.
- **`[8]` — the skills the agents cite** — `task-complexity-classifier`,
  `architecture-designer`, `implementation-planner`, `strict-implementer`, `logic-reviewer`,
  `style-reviewer`, `acceptance-validator`, `unit-test-writer` — at
  `~/.pi/agent/skills/<name>/SKILL.md`. Pi also finds them in `~/.agents/skills` and
  `<clone>/.agents/skills`, one grouping folder deep, and the doctor accepts all of them.
  It reads the names out of `.pi/agents/*.md`, so it stays correct when an agent starts
  citing another skill.
- **`[9a]`, `[9b]`, `[9c]` — optional, never fatal:** `graft` on `PATH` plus a graph to
  query — this checkout's `graft/INDEX.md`, or the primary checkout's when the doctor runs in
  a task worktree, because that is the one the lead reads (`graft build` creates it);
  the `cargo deny check` gate, which the doctor asks cargo for: cargo resolves the
  `cargo-deny` subcommand through `$CARGO_HOME/bin` even when that directory is not on
  `PATH`; and `CARGO_TARGET_DIR=/tmp/apt-scout-target` exported so worktrees share
  incremental builds. `cargo-deny` is a warning while Docker is a failure because CI runs
  `cargo deny check` as a required status check, so the gate is still verified there;
  `scripts/docker-test.sh` has no CI counterpart at all.

### Project trust

`.pi/` here holds agent files and this README, and none of the resources that make a project
ask for trust (`settings.json`, `mcp.json`, `extensions`, `skills`, `prompts`, `themes`,
`SYSTEM.md`, `APPEND_SYSTEM.md`, or an `.agents/skills` folder above the launch directory).
Pi therefore shows no trust prompt for this checkout and loads its agents with no recorded
entry. A *missing* entry is a `WARN` and not a failure, because nothing is broken and no
prompt will appear to fix it.

A recorded decision is a different matter: Pi reads `~/.pi/agent/trust.json` either way, so a
recorded `false` disables the project's agents and settings on a checkout that needs no
decision at all, and the doctor fails that case (`agentScope: "project"` refuses outright).
Recording `true` (`/trust` in Pi, or `"<absolute path>": true` in `~/.pi/agent/trust.json`)
is worth the one command for the opposite reason: it makes the outcome deterministic instead
of implied, and it stops a declined prompt — or `defaultProjectTrust: "never"` — from
silently disabling the team the day the checkout gains a trust-requiring resource, for
example a project `.pi/settings.json` or `.agents/skills`. Keys are absolute and are matched
against the launch directory after symlinks are resolved, so record the real path: a key
written as an unresolved symlink (a `/tmp` path on macOS, for instance) never matches.

From the moment such a resource exists, the same check changes what it says: it names the
resource and states that the decision is now required, because Pi prompts at launch and a
declined or impossible prompt (`pi -p`) leaves the team disabled.

## Model

The agent files carry no model pin on purpose. A child inherits the model of the session
that launches it, so the model is a machine-level setting rather than a repository rule:
set it in `~/.pi/agent/AGENTS.md` (the file `[3c]` checks for) and the whole team follows.
This machine currently runs `deepseek-flash` as its default model — an example of a working
setup, not a requirement.

## Not verified — check by hand

A green run covers the machine state a script can see. These are outside it, and the doctor
says nothing about them:

- **The git remote's push credentials.** `[7d]` reads `gh`'s permission on the repository,
  which is a different credential from the one `git push` uses (an SSH key, a credential
  helper, a token in the URL). Push a throwaway branch once to be sure.
- **The Git guard.** Some developer machines enforce a repository-kind setting and a matching
  identity and reject the commit otherwise; that is machine configuration, not repository
  content, and it fails at commit time rather than in the preflight.
- **The Pi version.** `[3a]` reports it and nothing pins it. The trust and agent-discovery
  behaviour described above was read from Pi 1.5.2's own code (`trust-manager.js`,
  `project-trust.js`); a Pi release that changes those rules changes this document.
- **`~/.pi/agent/settings.json`.** The doctor proves the extension is installed, not that it
  is enabled, and not what `defaultProjectTrust` or the default model is set to.
- **Whether the required gates produced a verdict.** The doctor proves the tooling exists,
  not that it ran: `[9b]` says `cargo deny check` can run here, `[6]` says the daemon answers,
  and neither is a result. A green doctor is not evidence that the licence and advisory scan
  or the container suite passed.
- **The trust prompt itself.** `[4a]`/`[4b]` report the decision Pi would read; they do not
  launch an interactive session to observe a prompt being shown or declined.

## Verifying the setup

```bash
bash scripts/agent-doctor.sh   # exit 0 when the team can run here
```

Run it in the clone you are about to launch Pi in, and again in a task worktree if you plan
to launch Pi there: the trust decision is recorded per path, so a decision recorded for the
clone does not cover the worktree. That costs nothing — a worktree needs no decision either,
for the same reason as the clone — and the doctor says so instead of failing.
