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

**Status: IMPLEMENTED; AUDIT FINDINGS OPEN** — feature work merged through PRs #13–#16 plus D6a in PR #18 and D6b in PR #19. D6f and D6e block all apt writes.

- [x] D1. Detect Ubuntu family, release codename, and supported architecture. [SPEC: Scope, Pipeline 1]
- [x] D2. Parse and validate Launchpad mirror candidates, with `mirrors.ubuntu.com` fallback. [SPEC: Pipeline 3–4]
- [x] D3. Probe mirrors, measure freshness/speed, rank, and expose read-only CLI. [SPEC: Pipeline 6–9]
- [x] D4. Detect country/AS optionally; support `--no-geo`; add bounded, validated Team Cymru AS lookups and AS tie-break. [SPEC: Pipeline 2, 5]
- [x] D5. Add local Docker runtime coverage and CI checks. [SPEC: Testing]
- [x] D6. Formal Oracle audit completed. Accepted requirements recorded in SPEC and tasks D6a–D6c. [SPEC: Pipeline preflight, Pipeline 9, Security note, Apply testing]
- [x] D6a. Reorder preflight so invalid/unknown `--country` and unsupported architecture fail before geo, HTTP, or DNS; Docker regression proves both cases are rejected without network attempts. Merged PR #18.
- [x] D6b. Show mirror AS and same/different/unknown status in CLI output; tests cover status mapping and scheme/port handling. Merged PR #19.
- [x] D6c. Fail-closed `InRelease` verification with fixed `/usr/bin/gpgv` and the Ubuntu archive keyring; `verified`/`invalid`/`unavailable` are shown without blocking read-only ranking, the `.deb` depends on `gpgv` and `ubuntu-keyring`, and README, AGENTS, SPEC, and the Docker suite document the boundary. Merged PR #25 as `665f1ed` (reviewed head `0207555`); on that head fmt, Clippy `-D warnings`, `cargo test --locked` (110 passed), `cargo deny check`, and Docker 19/19 (Ubuntu 20.04–26.04 offline and live, the missing-`gpgv` case, and the earlier negative cases) exited 0 and CI `changes`, `pr-title`, `docs-and-conventions`, `check`, `deny`, and `musl-build` were SUCCESS. The code review returned `CHANGES REQUESTED` and the security audit returned `FAIL` with open P1 findings, and the human authorized that merge directly instead of through the loop's merge condition; those P1 findings stay open in D6f. [SPEC: Security note, Apply]
- [ ] **D6e — READY.** Oracle threat-model/design audit plus independent security review of the proposed privilege boundary; approve a trusted root handoff and security acceptance tests before any privileged implementation. Explicitly decide whether self-reexec is allowed. Must also rule on the two D6f P1s, the signed-region `Date` and the verified-baseline representation, because both decide what a privileged write may trust. [SPEC: Privilege boundary and threat model]
- [ ] **D6f — READY.** Fix the open D6c findings before any privileged write. P1 (security): the freshness `Date` is parsed from the whole downloaded body, but `gpgv` ignores everything before `-----BEGIN PGP SIGNED MESSAGE-----`, so a mirror can prepend an attacker-chosen `Date:` to a genuinely signed body and still be reported `verified` with a forged date (proved by hand: `gpgv` exits 0 with `GOODSIG`/`VALIDSIG` on a prefixed copy of the real fixture); parse only inside the cleartext-signed region, reject non-whitespace before the armour, and add a prefixed-fixture regression test, with the corresponding requirement in SPEC step 8 and the Freshness trust boundary. P1 (security): the accepted rule that the selected mirror and the reference baseline must both be `Verified` has no representation, because `fresh::Probed` carries no signature, `Reference` is accepted without checking, and `fallback_baseline` takes any `Ok` outcome; add the type-level gate. P1 (review): no test asserts the `updates_signature` returned by the probe wiring, so replacing the verifier call with a constant keeps all 110 unit tests green, and the Docker live case only matches the word `signature`, which the table header already satisfies; add the assertions. Accepted P2s, not blocking: the `TMPDIR`-derived parent directory is not validated (owner, mode, sticky bit); `--homedir` is the directory that also holds the untrusted `InRelease`, so use a subdirectory or stdin; `reader.join()` is unbounded after the timeout kill; `SignatureStatus::Verified` is constructible anywhere in the crate, so a `VerifiedPlan` or private constructor belongs with the write gate; the production `GPGV`/`UBUNTU_KEYRING` constants are never exercised by a test and `test_gpgv()` skips silently; an oversized body is reported as `Unavailable` rather than `Invalid`; the `latency` column header now means time to first byte; SPEC step 6 and the Testing section still describe the old reference fetch and the old Docker/fixture set, and `GLOSSARY.md` lacks the user-facing `verified`/`invalid`/`unavailable` vocabulary. Depends on D6c; blocks D9 and every later write; D6e must rule on both P1s. [SPEC: Pipeline 6–8, Freshness trust boundary, Testing]


Merged PRs: #13, #14, #15, #16, D6a PR #18, D6b PR #19, D7 PR #21, D8 PR #23, and D6c PR #25 (merge commit `665f1ed`). D6a evidence: 79 unit tests and Docker 17/17; D6b evidence: 80 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker 17/17; D7 evidence: 92 unit tests and Docker 18/18. D8 evidence: 97 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker 18/18; PR #23 required CI and final review passed. D6c evidence on reviewed head `0207555`: 110 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker 19/19, and all six required checks SUCCESS, but the code review returned `CHANGES REQUESTED` and the security audit `FAIL` with open P1 findings, so that merge rests on human authorization rather than a clean verdict, and D6f carries the findings. PR #26 is a delivery-team chore and is not counted here. Oracle audit run 2026-11-12; see D6a–D6f.

## Milestone 2 — Apt source discovery, apply, backup, restore

**Status: IN PROGRESS** — D6a/D6b, D7, and D8 are complete, and D6c signature verification merged as PR #25 with two open P1 findings; D6c, D6e, and D6f gate every privileged write.

- [x] D7. Read-only apt source inventory and legacy/deb822 parsers; print eligible/skipped URIs and stop before probes if no eligible source exists. No apt writes. Tests cover comments/options, malformed/duplicate/missing fields, disabled/security stanzas, current mirrors, Pop!_OS mixed security suites, invalid URI ports/userinfo, root/file/directory symlinks, non-UTF8 files, third-party-only sources, and Docker CLI output. Merged PR #21.
- [x] D8. Pure, byte-preserving rewrite plan for legacy/deb822 URI tokens; returns transformed contents and staged Pop!_OS security file without filesystem IO. Tests cover CRLF/comments/options, multi-URI/disabled/security stanzas, Pop split, round trips, and invalid selected URLs. Merged PR #23.
- [ ] **D9 — BLOCKED by D6c, D6e, and D6f.** Implement safe backup manifest and atomic apply: no symlink following, preserve owner/mode, fsync, abort before writes on any backup failure. [SPEC: Apply]
- [ ] **D10 — BLOCKED by D9, D6e, and D6f.** Implement validated `--restore` from newest backup; test malformed manifests, path traversal, missing files, and rollback behavior. [SPEC: Apply]
- [ ] **D11 — BLOCKED by D10, D6c, D6e, and D6f.** Add CLI dry-run diff and explicit root/confirmation behavior; Docker-test actual source rewrite and restore in Ubuntu images. [SPEC: CLI, Apply, Testing]
- [ ] **D12 — BLOCKED by D6c, D6e, D6f, and D9–D11.** Oracle milestone audit; reconcile gaps in `SPEC.md`; update roadmap.

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
- [ ] **D20 — BLOCKED by D19.** Write and package the comprehensive Linux man page at `man/apt-scout.1`, with `man apt-scout`, all implemented options/modes, security/privacy, files/errors/exit status, and practical examples/recipes. Include it in `.deb` as `/usr/share/man/man1/apt-scout.1` and in the release `.tar.gz` with tarball installation instructions; validate formatting and query it in an Ubuntu container. [SPEC: CLI, Testing]
- [ ] **D21 — BLOCKED by D20 and D16.** Re-run laptop visual inspection and Ubuntu Docker tests; Oracle final audit against original goal and all of `SPEC.md`.

## Change log

- 2026-11-12: PR #17 established this roadmap and cycle. Formal Milestone 1 Oracle audit found preflight ordering and AS output gaps, plus the unsigned-InRelease write trust boundary. D6a merged as PR #18, D6b as PR #19, and read-only D7 as PR #21. D8 pure rewrite merged as PR #23. D6c signature verification merged as PR #25 (`665f1ed`) with two open P1 findings, so D6f now gates every write alongside D6c and D6e. After D19 stabilizes CLI/TUI behavior, D20 adds the detailed packaged `man apt-scout` manual before D21 final audit. All apt-write tests must run in disposable Ubuntu containers with isolated `/etc/apt`; never mount the host path.
