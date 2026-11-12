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

**Status: IMPLEMENTED; AUDIT REQUIRED** — feature work merged through PRs #13–#16, but the new formal Oracle milestone audit is still outstanding.

- [x] D1. Detect Ubuntu family, release codename, and supported architecture. [SPEC: Scope, Pipeline 1]
- [x] D2. Parse and validate Launchpad mirror candidates, with `mirrors.ubuntu.com` fallback. [SPEC: Pipeline 3–4]
- [x] D3. Probe mirrors, measure freshness/speed, rank, and expose read-only CLI. [SPEC: Pipeline 6–9]
- [x] D4. Detect country/AS optionally; support `--no-geo`; add bounded, validated Team Cymru AS lookups and AS tie-break. [SPEC: Pipeline 2, 5]
- [x] D5. Add local Docker runtime coverage and CI checks. [SPEC: Testing]
- [ ] D6. Run the first formal Oracle audit against the original goal, `SPEC.md`, delivered code, tests, and real-system evidence; record accepted gaps in `SPEC.md` and revise the next milestone backlog. [SPEC: all]

Merged PRs: #13, #14, #15, #16. Latest local evidence: 77 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, and Docker matrix 15/15 passed before PR #16 merge; PR #16 required CI and final review passed. Formal milestone audit has not run yet.

## Milestone 2 — Apt source discovery, apply, backup, restore

**Status: BLOCKED** — wait for D6 Oracle audit; D7 remains the first planned implementation task unless the audit changes scope or order.

- [ ] D7. Build source-file inventory and pure parsers for legacy `.list` and deb822 `.sources`; identify official Ubuntu archive URI tokens only, including Pop!_OS rules. Show inspected/ignored entries. [SPEC: Apply, no-rewritable-URI rule]
- [ ] D8. Implement pure URI rewrite with byte-preserving behavior for comments, options, non-target repositories, disabled stanzas, and security suites. Add fixture and round-trip tests. [SPEC: Apply]
- [ ] D9. Implement safe backup manifest and atomic apply: no symlink following, preserve owner/mode, fsync, abort before writes on any backup failure. [SPEC: Apply]
- [ ] D10. Implement validated `--restore` from newest backup; test malformed manifests, path traversal, missing files, and rollback behavior. [SPEC: Apply]
- [ ] D11. Add CLI dry-run diff and explicit root/confirmation behavior; Docker-test actual source rewrite and restore in Ubuntu images. [SPEC: CLI, Apply, Testing]
- [ ] D12. Oracle milestone audit; reconcile gaps in `SPEC.md`; update roadmap.

## Milestone 3 — TUI core and laptop inspection

**Status: PENDING** — blocked by D12 and completion of Milestone 2.

- [ ] D13. Implement TUI core with ranked mirror table, details, selection, confirmation, cancellation, write result, and terminal restoration. [SPEC: TUI]
- [ ] D14. Add `ratatui::TestBackend` snapshots and non-TTY/`--no-tui`/`--list` coverage. [SPEC: CLI, TUI, Testing]
- [ ] D15. Provide a laptop command/script that builds and launches the TUI against safe fixture/mock data, without touching host apt files; document how the user inspects it. Then verify program runtime in local Ubuntu Docker emulation. [SPEC: TUI, Testing]
- [ ] D16. User visually inspects the laptop terminal UI; capture feedback and fix accepted issues. Do not claim visual acceptance until user confirms.
- [ ] D17. Oracle milestone audit; update SPEC and roadmap.

## Milestone 4 — TUI custom mirror operations

**Status: PENDING** — blocked by D17 and completion of Milestone 3.

- [ ] D18. Add custom URL (`Insert`) validation and isolated probe flow. [SPEC: TUI, Apply]
- [ ] D19. Add current-run candidate removal (`Delete`) and confirmation. [SPEC: TUI]
- [ ] D20. Re-run laptop visual inspection and Ubuntu Docker tests; Oracle final audit against original goal and all of `SPEC.md`.

## Change log

- 2026-11-12: Milestone 1 recorded complete from merged PRs #13–#16. Milestone 2 is next; no apply/write behavior is implemented yet.
