# Delivery Roadmap

This file is the execution ledger for `SPEC.md`. `SPEC.md` defines behavior; this file defines work order and records delivery evidence. Keep entries current after each merged PR. Do not duplicate detailed design from the spec here.

## Working cycle

For each task, in order:

1. Read `AGENTS.md`, this roadmap, and the referenced `SPEC.md` sections. Confirm the target branch is current and the worktree is clean.
2. Before behavior changes, update `SPEC.md` if the required behavior is missing or incorrect. Split the task into bounded acceptance criteria and tests here.
3. Implement one coherent task on one feature branch. Parallel agents are allowed only for independent work in isolated branches/worktrees; never allow concurrent writers to the same files or shared worktree.
4. Run focused tests while changing code, then all required local checks, including `scripts/docker-test.sh` for Rust/build changes. Open a PR to `develop`.
5. Run an independent reviewer on the exact PR diff, fix or explicitly answer findings, wait for all required CI checks, then merge. Never push directly to `develop` or `main`.
6. Immediately record merged PR, verification, and next task in this file; start the next unblocked task without waiting for another prompt.
7. At each milestone boundary, run an Oracle audit against the original goal, `SPEC.md`, delivered code, tests, and known real-system behavior. Oracle may propose missing requirements; add accepted requirements to `SPEC.md`, then update this roadmap before implementation resumes.

Use task states `DONE`, `READY`, `BLOCKED`, and `PENDING`. `DONE` requires merged PR and passing verification. Keep task records concise and link them to `SPEC.md` sections. A task discovered during implementation becomes a new roadmap entry before it is forgotten.

## Goal

Deliver a safe Ubuntu-family CLI that discovers and ranks mirrors, lets the user inspect the result in a TUI, and safely applies/restores apt source changes. The TUI must be runnable on the user's laptop for visual inspection; Ubuntu runtime behavior must be exercised locally in Docker emulation.

## Milestone 1 — Read-only discovery and ranking

**Status: IMPLEMENTED; AUDIT FINDINGS OPEN** — feature work merged through PRs #13–#16 plus D6a in PR #18 and D6b in PR #19. D6c blocks all apt writes.

- [x] D1. Detect Ubuntu family, release codename, and supported architecture. [SPEC: Scope, Pipeline 1]
- [x] D2. Parse and validate Launchpad mirror candidates, with `mirrors.ubuntu.com` fallback. [SPEC: Pipeline 3–4]
- [x] D3. Probe mirrors, measure freshness/speed, rank, and expose read-only CLI. [SPEC: Pipeline 6–9]
- [x] D4. Detect country/AS optionally; support `--no-geo`; add bounded, validated Team Cymru AS lookups and AS tie-break. [SPEC: Pipeline 2, 5]
- [x] D5. Add local Docker runtime coverage and CI checks. [SPEC: Testing]
- [x] D6. Formal Oracle audit completed. Accepted requirements recorded in SPEC and tasks D6a–D6c. [SPEC: Pipeline preflight, Pipeline 9, Security note, Apply testing]
- [x] D6a. Reorder preflight so invalid/unknown `--country` and unsupported architecture fail before geo, HTTP, or DNS; Docker regression proves both cases are rejected without network attempts. Merged PR #18.
- [x] D6b. Show mirror AS and same/different/unknown status in CLI output; tests cover status mapping and scheme/port handling. Merged PR #19.
- [ ] **D6c — BLOCKED by D8.** Decide keyring/signature implementation and add fail-closed `InRelease` signature checks before any apply decision. No apt write task may start before D6c is implemented. [SPEC: Security note, Apply]


Merged PRs: #13, #14, #15, #16, D6a PR #18, D6b PR #19, and D7 PR #21. D6a evidence: 79 unit tests and Docker 17/17; D6b evidence: 80 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker 17/17; both passed required CI and review. D7 evidence: 92 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker 18/18; required CI and final review passed. Oracle audit run 2026-11-12; see D6a–D6c.

## Milestone 2 — Apt source discovery, apply, backup, restore

**Status: IN PROGRESS** — D6a and D6b are complete. D7 is complete; D8 is next and pure/read-only. Apt writes remain separately blocked by D6c.

- [x] D7. Read-only apt source inventory and legacy/deb822 parsers; print eligible/skipped URIs and stop before probes if no eligible source exists. No apt writes. Tests cover comments/options, malformed/duplicate/missing fields, disabled/security stanzas, current mirrors, Pop!_OS mixed security suites, invalid URI ports/userinfo, root/file/directory symlinks, non-UTF8 files, third-party-only sources, and Docker CLI output. Merged PR #21.
- [ ] **D8 — READY.** Implement pure URI rewrite with byte-preserving behavior for comments, options, non-target repositories, disabled stanzas, and security suites. Add fixture and round-trip tests. [SPEC: Apply]
- [ ] **D9 — BLOCKED by D8 and D6c.** Implement safe backup manifest and atomic apply: no symlink following, preserve owner/mode, fsync, abort before writes on any backup failure. [SPEC: Apply]
- [ ] **D10 — BLOCKED by D9.** Implement validated `--restore` from newest backup; test malformed manifests, path traversal, missing files, and rollback behavior. [SPEC: Apply]
- [ ] **D11 — BLOCKED by D10 and D6c.** Add CLI dry-run diff and explicit root/confirmation behavior; Docker-test actual source rewrite and restore in Ubuntu images. [SPEC: CLI, Apply, Testing]
- [ ] **D12 — BLOCKED by D6c and D8–D11.** Oracle milestone audit; reconcile gaps in `SPEC.md`; update roadmap.

## Milestone 3 — TUI core and laptop inspection

**Status: PENDING** — blocked by D12 and completion of Milestone 2.

- [ ] **D13 — BLOCKED by D12.** Implement TUI core with ranked mirror table, details, selection, confirmation, cancellation, write result, and terminal restoration. [SPEC: TUI]
- [ ] **D14 — BLOCKED by D13.** Add `ratatui::TestBackend` snapshots and non-TTY/`--no-tui`/`--list` coverage. [SPEC: CLI, TUI, Testing]
- [ ] **D15 — BLOCKED by D14.** Provide a laptop command/script that builds and launches the TUI against safe fixture/mock data, without touching host apt files; document how the user inspects it. Then verify program runtime in local Ubuntu Docker emulation. [SPEC: TUI, Testing]
- [ ] **D16 — BLOCKED by D15.** User visually inspects the laptop terminal UI; capture feedback and fix accepted issues. Do not claim visual acceptance until user confirms.
- [ ] **D17 — BLOCKED by D16.** Oracle milestone audit; update SPEC and roadmap.

## Milestone 4 — TUI custom mirror operations

**Status: PENDING** — blocked by D17 and completion of Milestone 3.

- [ ] **D18 — BLOCKED by D17.** Add custom URL (`Insert`) validation and isolated probe flow. [SPEC: TUI, Apply]
- [ ] **D19 — BLOCKED by D18.** Add current-run candidate removal (`Delete`) and confirmation. [SPEC: TUI]
- [ ] **D20 — BLOCKED by D19.** Re-run laptop visual inspection and Ubuntu Docker tests; Oracle final audit against original goal and all of `SPEC.md`.

## Change log

- 2026-11-12: PR #17 established this roadmap and cycle. Formal Milestone 1 Oracle audit found preflight ordering and AS output gaps, plus the unsigned-InRelease write trust boundary. D6a merged as PR #18, D6b as PR #19, and read-only D7 as PR #21. D8 pure rewrite is READY; D6c blocks all apt writes. All apt-write tests must run in disposable Ubuntu containers with isolated `/etc/apt`; never mount the host path.
