# `.pi/` — the apt-scout delivery team

Project configuration for the Pi agents that deliver this crate. Pi reads it only after
it grants **project trust**. Everything here is the part of the delivery setup that
lives in the repository; the rest is machine state, and `scripts/agent-doctor.sh`
verifies it.

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

`bash scripts/agent-doctor.sh` prints `PASS`/`FAIL` per item, the fix for every failure,
and exits non-zero while something required is missing. In prose:

1. **This clone with its five agent files.** Restore a missing one with
   `git checkout -- .pi/agents/`. Each file's frontmatter `name:` must equal its
   filename; a mismatch breaks delegation by name.
2. **Pi on `PATH` and the `pi-subagents` extension installed.**
   `pi install npm:pi-subagents`. Without that extension there is no subagent tool and
   the lead cannot delegate at all.
3. **Project trust — the step that silently breaks everything.**
   `~/.pi/agent/trust.json` must map this clone's absolute path (or a parent folder) to
   `true`. Fix: launch `pi` in the clone and answer the trust prompt, or run `/trust`
   and pick "Trust". While trust is not granted Pi loads no project configuration: the
   five agents are never discovered, `.pi/agents/` is ignored without an error, and the
   team does not exist. Nothing else looks broken in that state, which is why the doctor
   treats it as a hard failure. Non-interactive runs (`pi -p`) cannot show the prompt, so
   they need `--approve` or a saved decision.
4. **Rust toolchain.** `command -v cargo rustfmt` and `cargo clippy --version` must work
   on the stable toolchain `rust-toolchain.toml` pins:
   `rustup toolchain install stable --component rustfmt --component clippy`.
5. **Docker daemon reachable** (`docker info`). Needed only by `scripts/docker-test.sh`,
   which is a pre-merge gate whenever `.rs` or build files change: start Docker Desktop,
   or `sudo systemctl start docker`.
6. **`gh` authenticated** (`gh auth login`) and `gh repo view` working from the clone —
   the loop opens and merges PRs through it.
7. **The skills the agents cite** — `task-complexity-classifier`,
   `architecture-designer`, `implementation-planner`, `strict-implementer`,
   `logic-reviewer`, `style-reviewer`, `acceptance-validator`, `unit-test-writer` — each
   at `~/.pi/agent/skills/<name>/SKILL.md`. The doctor reads the actual names out of
   `.pi/agents/*.md`, so it stays correct when an agent starts citing another skill.
8. **Optional, never fatal:** `graft` on `PATH` (the lead reads structure from the graph
   instead of grepping), `cargo-deny` on `PATH` (the license and advisory gate), and
   `CARGO_TARGET_DIR=/tmp/apt-scout-target` exported so worktrees share incremental
   builds. The doctor reports these as `WARN`.

## Model

The agent files carry no model pin on purpose. A child inherits the model of the session
that launches it, so the model is a machine-level setting rather than a repository rule:
set it once in Pi and the whole team follows. This machine currently runs
`deepseek-flash` as its default model — an example of a working setup, not a requirement.

## Verifying the setup

```bash
bash scripts/agent-doctor.sh   # exit 0 when the team can run here
```

Run it in the clone you are about to launch Pi in. A linked worktree is a different
directory with its own trust decision, so there the doctor fails on project trust until
that path is trusted as well — correctly, because Pi launched in the worktree would load
no project agents either.
