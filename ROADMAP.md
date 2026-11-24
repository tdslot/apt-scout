# Delivery Roadmap

This file is the execution ledger for `SPEC.md`.
  `SPEC.md` defines behavior; this file defines work order and records delivery evidence.
  Keep entries current after each merged PR.
  Do not duplicate detailed design from the spec here.

The gates, verdicts and logs behind each task live in `docs/history/`, one file per milestone,
linked from the task that earned them.

## Working cycle

For each task, in order:

1. Read `AGENTS.md`, this roadmap, and the referenced `SPEC.md` sections.
   Confirm the target branch is current and the worktree is clean.
2. Before behavior changes, update `SPEC.md` if the required behavior is missing or incorrect.
   Split the task into bounded acceptance criteria and tests here.
3. Implement one coherent task on one feature branch.
   Parallel agents are allowed only for independent work in isolated branches/worktrees; never allow concurrent
   writers to the same files or shared worktree.
4. Run focused tests while changing code, then all required local checks, including `scripts/docker-test.sh` for
   Rust/build changes.
   Open a PR to `develop`.
5. Run an independent reviewer on the exact PR diff, fix or explicitly answer findings, wait for all required CI
   checks, then merge.
   Never push directly to `develop` or `main`.
6. Immediately record merged PR, verification, and next task in this file; start the next unblocked task without
   waiting for another prompt.
7. At each milestone boundary, run an Oracle audit against the original goal, `SPEC.md`, delivered code, tests, and
   known real-system behavior.
   Oracle may propose missing requirements; add accepted requirements to `SPEC.md`, then update this roadmap before
   implementation resumes.

Use task states `DONE`, `READY`, `BLOCKED`, `PENDING`, and the two the scope rulings
  added: `DEFERRED by Dn` when a ruling moves a task out of v1, and `CLOSED by Dn, not planned` when it is
  dropped. A box is `[x]` when no further work is expected —
  landed or closed — and `[ ]` when work remains, a deferral included.
  `DONE` requires merged PR and passing verification.
  Keep task records concise and link them to `SPEC.md` sections.
  A task discovered during implementation becomes a new roadmap entry before it is forgotten.

## Goal

Deliver a safe Ubuntu-family CLI that discovers and ranks mirrors, lets the user inspect the result in a TUI, and
  safely applies/restores apt source changes. v1 prints **English only** (D79); the localization chapters
  stay the post-v1 design.
  The TUI must be runnable on the user's laptop for visual inspection; Ubuntu runtime behavior must be exercised
  locally in Docker emulation.

## Milestone 1 — Read-only discovery and ranking

**Status: IMPLEMENTED; OPEN FINDINGS TRACKED BELOW, NONE OF THEM A WRITE GATE** — what M1 delivered is the read-only
  product: Ubuntu-family/codename/architecture detection, Launchpad and `mirrors.ubuntu.com` discovery, URL and DNS
  hygiene, optional country/AS with Team Cymru ranking, `-updates`/`-security` freshness, the parallel probe and the
  ranker, and the read-only CLI (`--list`, `--any`, `--country`, `--no-geo`, `--top`, `--https-only`, `--verbose`,
  `--no-color` and, since PR #63, `--version`), merged through PRs #13–#16, D6a in PR #18 and D6b in PR #19.
  Its audit produced **D6a–D6c, D6e and D6f** — five task ids, no sixth — and those are closed: D6c's two P1s by
  D6f
  in PR #29, the boundary itself by D6e in PR #28.
  The findings still open under this milestone are ledger items with owners, not a block on any write: D22, D29 and
  D46.
  **The write path does not wait on D6e and has not since PR #43:** D6e is `DONE` with PR #28, the writer merged as
  D9b (PR #36) and reached the CLI as D11a (PR #43), the restore engine followed as D10a/D10b (PR #59) and its flag as
  D11b (PR #62), and the follow-ups the record left open closed with D25 (PR #43) and D49 (PR #64).
  D6f merged as PR #29.

- [x] D1.
  Detect Ubuntu family, release codename, and supported architecture. [SPEC: docs/spec/platform.md,
  docs/spec/pipeline.md 1]
- [x] D2.
  Parse and validate Launchpad mirror candidates, with `mirrors.ubuntu.com` fallback. [SPEC: docs/spec/pipeline.md
  3–4]
- [x] D3. Probe mirrors, measure freshness/speed, rank, and expose read-only CLI. [SPEC: docs/spec/pipeline.md 6–9]
- [x] D4.
  Detect country/AS optionally; support `--no-geo`; add bounded, validated Team Cymru AS lookups and AS tie-break.
  [SPEC: docs/spec/pipeline.md steps 2 and 5]
- [x] D5. Add local Docker runtime coverage and CI checks. [SPEC: docs/spec/testing.md]
- [x] D6.
  Formal Oracle audit completed.
  Accepted requirements recorded in SPEC and tasks D6a–D6c. [SPEC: docs/spec/pipeline.md preflight,
  docs/spec/pipeline.md 9, docs/spec/non-functional.md security note, docs/spec/apply.md testing]
- [x] D6a.
  Reorder preflight so invalid/unknown `--country` and unsupported architecture fail before geo, HTTP, or DNS; Docker
  regression proves both cases are rejected without network attempts.
  Merged PR #18.
- [x] D6b.
  Show mirror AS and same/different/unknown status in CLI output; tests cover status mapping and scheme/port handling.
  Merged PR #19.
- [x] D6c.
  Fail-closed `InRelease` verification with fixed `/usr/bin/gpgv` and the Ubuntu archive keyring;
  `verified`/`invalid`/`unavailable` are shown without blocking read-only ranking, the `.deb` depends on `gpgv` and
  `ubuntu-keyring`, and README, AGENTS, SPEC, and the Docker suite document the boundary.
  Merged PR #25 as `665f1ed` (reviewed head `0207555`); on that head fmt, Clippy `-D warnings`, `cargo test --locked`
  (110 passed), `cargo deny check`, and Docker 19/19 (Ubuntu 20.04–26.04 offline and live, the missing-`gpgv` case,
  and the earlier negative cases) exited 0 and CI `changes`, `pr-title`, `docs-and-conventions`, `check`, `deny`, and
  `musl-build` were SUCCESS.
  The code review returned `CHANGES REQUESTED` and the security audit returned `FAIL` with open P1 findings, and the
  human authorized that merge directly instead of through the loop's merge condition; those P1 findings stay open in
  D6f. [SPEC: docs/spec/non-functional.md security note, docs/spec/apply.md]
- [x] **D6e — DONE.** Oracle threat-model/design audit plus independent security review of the privilege boundary,
  approving the boundary and its acceptance tests before any privileged implementation, and ruling on self-reexec. —
  [evidence](docs/history/m1.md#d6e)
- [x] **D6f — DONE.** Fix the open D6c findings before any privileged write. — [evidence](docs/history/m1.md#d6f)


Merged PRs: #13, #14, #15, #16, D6a PR #18, D6b PR #19, D7 PR #21, D8 PR #23, D6c PR #25 (merge commit `665f1ed`),
  and D6f PR #29 (merge commit `8bbfa72`).
  D6a evidence: 79 unit tests and Docker 17/17; D6b evidence: 80 unit tests, fmt, Clippy `-D warnings`,
  `cargo deny check`, Docker 17/17; D7 evidence: 92 unit tests and Docker 18/18.
  D8 evidence: 97 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker 18/18; PR #23 required CI and
  final review passed.
  D6c evidence on reviewed head `0207555`: 110 unit tests, fmt, Clippy `-D warnings`, `cargo deny check`, Docker
  19/19, and all six required checks SUCCESS, but the code review returned `CHANGES REQUESTED` and the security audit
  `FAIL` with open P1 findings, so that merge rests on human authorization rather than a clean verdict, and D6f
  carries the findings.
  PR #26 is a delivery-team chore and is not counted here.
  Oracle audit run 2026-11-12; see D6a–D6c, D6e and D6f.

## Milestone 2 — Apt source discovery, apply, backup, restore

**Status: COMPLETE ON THE EVIDENCE, WITH D54-D56 AND D24 OPEN** — the D12 audit found Milestone 2 feature-complete
  with its controls enforced in code (thirteen inventory rows: twelve with a control that refuses and a test that
  would fail if the control were removed, the thirteenth — the write path's isolation — proved by a source scan
  whose
  blind spot is D54), and filed three evidence-quality gaps and one documentation debt rather than closing over them.
  D6a/D6b, D7, and D8 are complete; D6c signature verification merged as PR #25 and its P1 findings were closed by D6f
  in PR #29, so D6e's privilege boundary stands and the security-acceptance gate is what the write tasks must satisfy.
  D9a (pure core), D9b (the writer) and D9c (the harness and the S-matrix subset it owns) are done, so the writer is
  exercised end to end in disposable containers.
  D9d closed the D9 record, D11a merged as PR #43 and made the writer reachable from the CLI, and D25 closed in that
  same PR before the wiring landed.
  What is left of the CLI is D11b's `--restore`; D10a and D10b build the restore it wires and are done (D10 was split
  on 2026-11-13 because the engine alone and the reader alone are each one PR).
  Above that, the surface a person actually touches is delivered and recorded here: D42's progress line and colour (PR
  #51), D44's `-v` threshold (PR #53), D47's `README.md` as the one user document (PR #58) and D48's `.deb` builder
  (PR #56) are `DONE` as of 2026-11-14, alongside D37's single report (PR #47), D38's short aliases (PR #49) and D43's
  `--any` with its one widening (PR #52).
  Two follow-ups the human found by testing the installed `.deb` are `DONE` in PR #63 (`7e36cc2`): D51's `--version`
  and `-V`, which was an unknown option, and D52's `sudo apt update` hint, which a root run printed without the
  privilege the reader no longer has.
  So the open work here is the two `.deb` builders D48 leaves in step by hand — no task owns changing the release
  path, because `.github/**` is closed to every delivery agent — the test rule D53 carries into the next test written
  or reviewed, and the `READY` and `PENDING` items this file already carried.
  D49, the other deferred finding, is `DONE` in PR #64 (`fa2a955`): `Plan::validate` now refuses a removal outside a
  restore plan.
  D50, the one PR #60's security read deferred, closed in the docs pass that wrote this line: the record's evidence
  commands assert their own output and its citations name their definitions.

- [x] D7.
  Read-only apt source inventory and legacy/deb822 parsers; print eligible/skipped URIs and stop before probes if no
  eligible source exists.
  No apt writes.
  Tests cover comments/options, malformed/duplicate/missing fields, disabled/security stanzas, current mirrors,
  Pop!_OS mixed security suites, invalid URI ports/userinfo, root/file/directory symlinks, non-UTF8 files,
  third-party-only sources, and Docker CLI output.
  Merged PR #21.
- [x] D8.
  Pure, byte-preserving rewrite plan for legacy/deb822 URI tokens; returns transformed contents and staged Pop!_OS
  security file without filesystem IO.
  Tests cover CRLF/comments/options, multi-URI/disabled/security stanzas, Pop split, round trips, and invalid selected
  URLs.
  Merged PR #23.
- [x] **D9a — DONE.** Pure core of the writer: `src/digest.rs`, `src/plan.rs`, `src/manifest.rs` — closed target
  enumeration, plan and manifest types, whole-plan validation, and the validating manifest parser.
  Unit tests only; nothing in the runtime calls it yet. — [evidence](docs/history/m2.md#d9a)
- [x] **D9b — DONE.** The writer in `src/apply.rs`: root check, state capture and disk binding, freshness gate,
  ownership gate, target validation, backup phase with manifest, and the atomic write path that preserves mode and
  owner. — [evidence](docs/history/m2.md#d9b)
- [x] **D9c — DONE.** The write harness, compiled out of every default and release build, plus the container matrix
  for S5/S6/S7/S8/S9/S11/S15/S16/S17 in disposable Ubuntu images with a container-local `/etc/apt`. —
  [evidence](docs/history/m2.md#d9c)
- [x] **D26 — DONE (#92), a human `chore:` PR.** Nothing compiled `src/harness.rs` in CI: the module is gated on
  `apt_scout_write_harness`, no workflow passed that cfg, and the only compiler that saw the file was
  `scripts/docker-test.sh`, which never runs in CI.
  The `check` job — already a required status — now runs clippy a second time with
  `RUSTFLAGS=--cfg apt_scout_write_harness`.
  Measured, not assumed: with a deliberate syntax error appended to `src/harness.rs`, plain clippy reports **0**
  errors and the cfg pass reports **3**, and with the file restored both are clean. docs/spec/testing.md states the
  pass, so the reason survives the commit message. [SPEC: docs/spec/testing.md]
- [x] **D27 — DONE.** `scripts/docker-test.sh` piped an unpinned remote script into a shell inside a container with
  the checkout mounted read-write; it now fetches the published `rustup-init`, verifies it against the `.sha256`, and
  runs that; the toolchain version stays whatever `rust-toolchain.toml` names. — [evidence](docs/history/m2.md#d27)
- [x] **D9d — DONE.** The D9 closeout: the writer's glossary terms, the ledger entry that was missing, and the record
  corrected where D9a/D9b/D9c had made it stale. [SPEC: docs/spec/apply.md] — [evidence](docs/history/m2.md#d9d)
- [ ] **D22 — PENDING, not on the critical path.** Split the Docker suite into a deterministic offline part that CI
  runs and a live part that stays local. — [evidence](docs/history/m2.md#d22)
- [x] **D23 — DONE.** Root `AGENTS.md` described `src/` as it was before D9a, which added `digest.rs`, `plan.rs` and
  `manifest.rs` while the root Project list named none of them. — [evidence](docs/history/m2.md#d23)
- [x] **D24 — DONE (#67), a human `chore:` PR; no delivery agent may do it.** The root `AGENTS.md` Project list is
  behind the tree in seven ways, all closed to every delivery agent: it still carries no `src/apply.rs` bullet and
  still says "Apply/source rewriting is not implemented yet", which D9b falsifies (the writer, its backup and the
  manifest path exist); and D9c added `src/harness.rs`, `build.rs` and a `scripts/docker-test.sh` that now does more
  than "container tests", none of which that list describes; and D43 added `-a`/`--any` with the one automatic
  widening, neither of which the `src/main.rs` bullet's flag list names; and `AGENTS.md:120`'s Tracking bullet still
  says "Testing: unit tests on fixtures; integration tests in Docker", which PR #63 falsifies by adding `tests/cli.rs`
  — a cargo integration test target that `cargo test` runs on the developer's machine, reading no fixture and needing
  no container. — [evidence](docs/history/m2.md#d24)
- [x] **D25 — DONE.** The write proof was forgeable from inside the process, so `docs/threat-model.md` 3.10 and
  Appendix A item 4 did not hold and S15's claim that the value cannot be constructed without the verification pass
  was false. — [evidence](docs/history/m2.md#d25)
- [x] **D28 — CLOSED by D81, not planned.** `--refresh` is in the `docs/spec/cli.md`
  flag table and there is no cache behind it, so the flag has nothing to refresh and is not parsed: a run given it
  fails as an unknown option with exit 2 (`rg -n -- '"--refresh"' src/` prints nothing), while docs/spec/pipeline.md
  step 2.3 describes a `~/.cache/apt-scout/mirrors.json` disk cache valid for 24 h, `Errors and self-healing` names
  its missing, empty and corrupt cases, and `Non-functional` lists the geo/mirror cache as planned work that no
  roadmap task owns. — [evidence](docs/history/m2.md#d28)
- [ ] **D29 — PENDING; start after D11b, which adds its restore cases to the same script.** A live-network container
  case lets a third-party mirror gate a merge. — [evidence](docs/history/m2.md#d29)
- [x] **D10a — DONE.** Make the writer able to express a restore's write set, so that `--restore` has a plan to hand
  it and one writer still writes. — [evidence](docs/history/m2.md#d10a)
- [x] **D10b — DONE.** The restore engine, no CLI: `src/restore.rs` builds a `Plan` from the newest backup alone, and
  `apply::apply(&plan, None, now)` is the writer it hands it to, so the writer keeps one entry point and no new one is
  added for restore. — [evidence](docs/history/m2.md#d10b)
- [x] **D11a — DONE.** The CLI write path: a root run builds the plan from the run's own inventory and rewrite output,
  prints it, asks one confirmation unless `--yes`, then calls `apply()` and prints the backup id and the files
  written; a non-root run prints the recommendation and the `sudo apt-scout …` line and exits 3; `--list` prints the
  table and the planned change and never writes; refusals exit 1. — [evidence](docs/history/m2.md#d11a)
- [x] **D11b — DONE.** The other half of the CLI: `--restore` parses and behaves by uid — a non-root run prints only
  `sudo apt-scout --restore`, exits 3 and opens no backup, a root run restores the newest backup after one
  confirmation (`--yes` skips it) — plus the container case that restores into an isolated `/etc/apt` and compares
  bytes, mode and owner (S4, S10, S11's manifest half). — [evidence](docs/history/m2.md#d11b)
- [x] **D35 — DONE.** A mirror a previous run wrote must survive a country change; ruled by the human 2026-11-13,
  found in a container. — [evidence](docs/history/m2.md#d35)
- [ ] **D36 — PENDING; ruled by the human 2026-11-13, not a dependency of D35.** When every mirror list fails but the
  network works, the canonical archive should still be a candidate, so a run does not end with nothing to do because a
  list is down.
  It is not today: `load_candidates` returns exit 1 "no mirror list is available" (`src/main.rs:1645`) before any
  probe, and the `archive.ubuntu.com` fetch in Pipeline 6 is the freshness reference, not a candidate.
  The task: add `http://archive.ubuntu.com/ubuntu/` as a last list source with a warning and a source name in the
  output, ranked and verified like any other candidate; it then either plans an empty change and exits 0 (already on
  the canonical archive) or writes it.
  Needs its own `SPEC.md` delta in Pipeline 3–4 and the exit-code section, plus the container case where both lists
  are blocked but the archive answers. [SPEC: docs/spec/pipeline.md 3–4, docs/spec/cli.md]
- [x] **D37 — DONE; the id was assigned when the merge was recorded, not before it.** A run prints one structured
  report instead of a table plus loose lines: a header, `Sources`, `Ranked — <n> of <m> usable, <k> rejected`,
  `Recommendation`, `Planned change`, and a `Rejected` section whose entries are grouped by the reason they were
  rejected (`src/main.rs:627`, `:873`, `:988`, `:1352`, `:1381`). — [evidence](docs/history/m2.md#d37)
- [x] **D38 — DONE; the id was assigned when the merge was recorded, not before it.** A short form for every flag that
  exists — `-c`, `-l`, `-y`, `-t`, `-n`, `-s`, `-v` and `-h` — with `-a`, `-r` and `-R` reserved for `--any`,
  `--restore` and `--refresh` and rejected until those flags exist; no bundling, so `-lyc NO` stays an unknown option
  (`src/main.rs:2068`); and the line a non-root run prints still spells the long forms, so a copied command is
  unambiguous. — [evidence](docs/history/m2.md#d38)
- [x] **D39 — DONE (#96), docs-only; found by PR #47's review.** The exit-code tables did not cover a case the program
  already has. docs/spec/non-functional.md's list and `--help`'s `EXIT CODES` block (`src/main.rs:237–239`) both
  defined 4 as unsupported distro or architecture, while `src/main.rs:2188–2194` exits 4 for a second, unrelated case:
  when the reference `-updates` `InRelease` answers 404 and the release has moved to `old-releases.ubuntu.com`, the
  run exits 4 with a message naming the codename. — [evidence](docs/history/m2.md#d39)
- [x] **D40 — DONE.** The dot-segment refusal recognises one encoding layer while `%252e` and `...` are ordinary
  segment names; ruled as a ruling, not a defect, and the entry says why keeping the check at one layer is not a hole.
  — [evidence](docs/history/m2.md#d40)
- [x] **D41 — DONE (#94).** The report shape lived in `--help` and in code comments, not in the contract.
  docs/spec/output.md now states the two facts a reader of the table needs and the code already had: the trailing
  `notes` cell carries exactly `redir:<host>`, cut to 16 columns and ending in `~` when it did not fit
  (`REDIR_HOST_MAX`, `CUT_MARKER` in `src/main.rs`), and nothing else — the `-security` lag moved to `Detail`; and a
  column that would be empty is dropped, the AS column when no shown row has AS data and the `notes` column when no
  shown row has a note.
  The old row text named `sec:<n>m` as cell content; that is no longer true of the product —
  `rg -n "sec:" src/main.rs` finds only the comment recording why it left — so the row records what the code does, not
  what it did.
- [x] **D60 — DONE (#TBD).** An independent documentation-versus-product audit and its fixes: a reviewer given
  *documentation must match the product 100%* tried to falsify every claim in `README.md`, the man page, `--help` and
  the user-facing `SPEC.md`. — [evidence](docs/history/m2.md#d60)
- [x] **D61 — DONE (#TBD).** The three product-text defects the D60 audit found, fixed in the product rather than
  documented around, starting with a closing line that printed `changed wrote <path>`. —
  [evidence](docs/history/m2.md#d61)
- [x] **D62 — DONE (#TBD).** The second documentation-versus-product audit: 6 mismatches, 5 documents and 1 product,
  after re-checking all 13 of D60 and finding them held. — [evidence](docs/history/m2.md#d62)
- [x] **D63 — DONE (#TBD).** The `Rejected` block prints a count per class, because three documents and `SPEC.md` said
  it did and the code did not. — [evidence](docs/history/m2.md#d63)


- [x] **D42 — DONE; the human's presentation request of 2026-11-13, from trying the tool.** Three changes, one PR: a
  transient progress line so a root run does not look hung, colour in the report, and three lines deleted.
  docs/spec/output.md is the contract and `docs/threat-model.md` Appendix A item 14 records the environment read; no
  ruling and no S1–S17 assertion moves, so no Gate 1 is owed. — [evidence](docs/history/m2.md#d42)
- [x] **D43 — DONE; ruled by the human 2026-11-13, and it is the last thing he asked for before this entry.** `--any`
  (`-a`) plus the automatic widening a run does when his own country has nothing usable, so a home country whose
  nearest mirror is down or incomplete costs him no fiddling. — [evidence](docs/history/m2.md#d43)
- [x] **D44 — DONE; the human's presentation ruling of 2026-11-13, from trying the tool, and it is one PR.** The
  default report shows only what a person deciding needs; every other fact moves behind `-v`/`--verbose`. —
  [evidence](docs/history/m2.md#d44)
- [x] **D47 — DONE, docs-only; merged after #56 and #57 and superseded their `README.md` hunks.** `README.md` becomes
  the one complete user document — what it is, requirements, install (the `.deb`, the tarball, from source), quick
  start, how a run works, what the output means, options, modes and exit codes, backups with the hand-restore path,
  network and privacy, troubleshooting, a documentation index and the licence — and it points at `SPEC.md` instead of
  restating it. — [evidence](docs/history/m2.md#d47)
- [x] **D48 — DONE; the id was assigned when the merge was recorded, not before it.** `scripts/build-deb.sh` builds
  the installable package from the static binary with no `cargo-deb` on the machine — which matters on macOS, where
  there is no `dpkg-deb` either, so the script falls back to an `ubuntu:24.04` container for the build. —
  [evidence](docs/history/m2.md#d48)
- [x] **D49 — DONE.** (P2 from PR #59's security read, deliberately deferred rather than fixed there.) **Every
  `src/plan.rs` line number in this bullet and the acceptance one is the file at `7e36cc2`, the head the task was
  written against; the fix inserts its own arm above these, so those numbers do not name the branch's file.**
  `Effect::Remove` is not bound to `Authority::Restore`. — [evidence](docs/history/m2.md#d49)
- [x] **D50 — DONE (P3s from PR #59's and PR #60's security reads, deferred from those PRs rather than fixed in
  them).** The record's evidence does not check itself. — [evidence](docs/history/m2.md#d50)
- [x] **D51 — DONE.** `apt-scout --version` was an unknown option: the installed `.deb` answered
  `error: unknown option '--version'; try --help` and exited 2, so a person with the package installed could not say
  which version they had without `dpkg -l`, and the bug report template at `.github/ISSUE_TEMPLATE/bug_report.yml:15`
  already asks them for that output.
  **The human found it while testing the installed package**, in the same demo that produced the `--help` check. —
  [evidence](docs/history/m2.md#d51)
- [x] **D52 — DONE.** The last line a write prints named a command the reader cannot run:
  `Next: run 'apt update' to refresh the package lists.` is printed by a root run started as `sudo apt-scout …`, and
  that process has exited by the time the line is read, so the person is back in their own unprivileged shell where
  `apt update` fails with permission denied. — [evidence](docs/history/m2.md#d52)
- [x] **D53 — DONE.** The rule is `TEST-1`: an assertion compares a value with an expectation that does not
  derive from the same expression, and a changed test is shown to fail. What was missing was enforcement, and
  the reviewer now demands the mutation named — [evidence](docs/history/m2.md#d53)
- [x] **D89 — DONE.** `escape::control_free` rewrites every control character on the four paths B7 names
  (`docs/threat-model.md:418`) and on the sites the audit found beyond them, and covers every Unicode `Cc`
  character, not ASCII alone. Tests pin the helper, the four paths and the sites D91 added; six other call sites are
  escaped and unpinned, which is **D92**. The escaping case proves it end to end: a served list whose URL carries
  `ESC` reaches the rejection note as text, with zero raw control bytes in the output. Merged as #121 (`e55a426`)
  and #125 (`8815f72`). — [evidence](docs/history/backlog.md#d89)
- [x] **D90 — DONE.** The seam is a second `#[cfg(apt_scout_write_harness)]` case, `__write-harness escaped-list`:
  it serves a Launchpad page from a constant compiled into the binary, runs the real `load_candidates` against
  it and prints what the run says. No environment read and no path, which is the harness doctrine already in
  `src/harness.rs` and S16 of the record, so no fourth environment read was needed. Case 17 of
  `scripts/docker-test.sh` runs it in `ubuntu:24.04` offline and fails on a raw control byte or on the
  character being absent; two mutations were run to prove both halves. `TEST-10` states it. Merged as #125 (`8815f72`).
  — [evidence](docs/history/backlog.md#d90)
- [x] **D91 — DONE.** Five tests pin the four sites the reviewer named on #121: the geolocation note, the
  fallback list warning, the `error:` line, and every sources-section row that interpolates a path, a host or a
  reason. The sixth row kind, an empty inventory, interpolates nothing and so carries nothing to pin. Three builders
  were extracted (`geolocation_note`, `fallback_warning`, `error_line`) so each prints through a
  function a test can call. `TEST-9` states the rule that keeps it: the count of `escape::control_free` sites
  is asserted by a test, so a new site fails the suite until it is covered. Nine mutations were run, each failing the
  suite, including the count-preserving one the review used to find the row this fixture had missed. Six sites
  elsewhere in `src/main.rs` are escaped and still unpinned, which is **D92**. Merged as #123 (`1f02289`).
  — [evidence](docs/history/backlog.md#d91)
- [x] **D92 — DONE.** The six sites the review of #123 found are pinned. Three builders came first, because a
  print inside `run` cannot be called from a test: `as_lookup_note`, `reference_warning` and
  `nothing_to_change_line`; and two for the planned diff, `added_line` and `removed_line`, which carry
  the new file bytes and both URIs. Five mutations were run, each failing the test that covers it, and `ESCAPE_SITES`
  moved from 37 to 36 because two call sites became one builder each. `TEST-9` and `OUT-12` are implemented now
  that nothing is escaped and unpinned. Merged as #127 (`e17e7f1`). — [evidence](docs/history/backlog.md#d92)
- [x] **D12 — DONE.** — [evidence](docs/history/m2.md#d12)

- [x] **D57 — DONE.** — [evidence](docs/history/m2.md#d57)

- [x] **D59 — DONE (#78), Ubuntu's carrot orange on the three places the proposal of 2026-11-14 named.** —
  [evidence](docs/history/m2.md#d59)

## Milestone 3 — TUI core and laptop inspection

**Status: PENDING** — blocked by D12 and completion of Milestone 2.

- [ ] **D13 — BLOCKED by D12.** Implement the `--tui` core with ranked mirror table, details, selection, confirmation,
  cancellation, write result, and terminal restoration.
  As root, confirm validates and writes; as non-root, confirm prints `sudo apt-scout --tui`.
  Every user-facing string the TUI adds goes through the message catalog of D30a **once localization returns**; with D79
  ruling v1 English only, D13 writes English strings at the call site and D30c's whole-tree literal test moves with the
  catalog work, and the TUI's modules join D30b's module split when the catalog work starts, so no string is
  unowned. A TUI that ships before the catalog is not a localization defect, it is the v1 scope D79 ruled.
  [SPEC: docs/spec/cli.md, docs/spec/tui.md, docs/spec/localization.md]
- [ ] **D14 — BLOCKED by D13.** Add `ratatui::TestBackend` snapshots and coverage of `--tui` as an opt-in mode: no TUI
  without the flag, non-TTY behavior, `--list` and the non-root `sudo` line. [SPEC: docs/spec/cli.md,
  docs/spec/tui.md, docs/spec/testing.md]
- [ ] **D15 — BLOCKED by D14.** Provide a laptop command/script that builds and launches the TUI against safe
  fixture/mock data, without touching host apt files; document how the user inspects it.
  Then verify program runtime in local Ubuntu Docker emulation. [SPEC: docs/spec/tui.md, docs/spec/testing.md]
- [ ] **D16 — BLOCKED by D15.** User visually inspects the laptop terminal UI; capture feedback and fix accepted
  issues.
  Do not claim visual acceptance until user confirms.
- [ ] **D17 — BLOCKED by D16.** Oracle milestone audit; update SPEC and roadmap.

## Milestone 4 — TUI custom mirror operations

**Status: PENDING** — blocked by D17 and completion of Milestone 3.

- [ ] **D18 — BLOCKED by D17.** Add custom URL (`Insert`) validation and isolated probe flow. [SPEC: docs/spec/tui.md,
  docs/spec/apply.md]
- [ ] **D19 — BLOCKED by D18.** Add current-run candidate removal (`Delete`) and confirmation. [SPEC:
  docs/spec/tui.md]
- [x] **D20 — DONE; pulled forward from after D19 on 2026-11-14, because the page documents only what ships.** The
  human installed the `0.1.0` `.deb` and `man apt-scout` answered "No manual entry": `SPEC.md:193` requires a
  comprehensive Linux man page at `man/apt-scout.1` and the package carried none. — [evidence](docs/history/m4.md#d20)
- [x] **D58 — DONE (#68), the two-builder gap D48 recorded.** — [evidence](docs/history/m4.md#d58)
- [x] **D45 — DONE (#68).** `SPEC.md:196` requires the page to be "include[d] in the release `.tar.gz` with
  installation instructions", and the archive now carries both halves without inventing a file: `release.yml` copies
  `man/apt-scout.1` beside the binary, and `README.md` — already in the archive — is the installation document, with
  the `.deb` path, the tarball path and the two packages a tarball install needs.
  The previous state is recorded in [the m4 history](docs/history/m4.md#d58): nothing copied the page at all until
  this PR.
- [x] **D46 — DONE.** The man page had no automated check; `scripts/docker-test.sh` now renders it in four Ubuntu
  images, runs `mandoc -Tlint`, and compares its `OPTIONS` tags with `--help` — [evidence](docs/history/m4.md#d46)
- [ ] **D21 — BLOCKED by D16, D33 and the last translation group D31j.** Re-run laptop visual inspection and Ubuntu
  Docker tests; Oracle final audit against original goal and all of `SPEC.md`, Localization and every shipped locale
  included.

## Milestone 5 — Localization and localized manuals

**Status: DEFERRED** — D79 ruled v1 English only, so every entry in this section is post-v1 and none of them
  blocks a release. Their dependencies are unchanged and they stay ordered as written, when the question comes back.
  The rules are docs/spec/localization.md and they bind every entry below.
  The mechanism is Fluent: one `locales/<tag>/main.ftl` per locale, compiled into the binary at build time by
  `fluent-templates`' `static_loader!` with English as the fallback language.

Shared acceptance, inherited by every entry here and not restated: docs/spec/localization.md read in full;
  `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` and
  `cargo deny check` each exit 0 with `CARGO_TARGET_DIR=/tmp/apt-scout-target`; the catalog gates of
  docs/spec/localization.md extended to every catalog the entry adds; the entry's tag added to the shipped set the
  tests require in `src/l10n.rs`, so a translation PR changes a `.rs` file too — without that, a `locales/`-only diff
  matches no path in the `changes` job of `.github/workflows/ci.yml` and `check` is skipped, which would let an
  incomplete catalog merge green; a container run of the read-only CLI under `LANG=C` and under the entry's locale(s),
  with the same exit code and byte-identical container-local `/etc/apt`, and the printed text differing exactly as
  that entry's catalogs differ — an entry that ships a translated catalog prints that locale's text, while D30a, which
  ships none, prints English in all three of its cases; and one rendered sample of `--list` in each added locale,
  pasted in the PR inside a fenced block exactly as the program printed it.
  That sample is the only non-English text a PR carries, and it is evidence rather than prose: the PR's own sentences
  stay English (docs/spec/non-functional.md), and D34 records the same exception where the rule lives.
  No translated text in any `.rs`, `.md`, `.toml` or `.yml` file: `docs-and-conventions` fails a file in those globs
  that carries Lithuanian letters, so translated text lives only in `locales/*/main.ftl` and `man/<locale>/`; `.ftl`
  is outside that glob list, which is why a catalog-only diff does not turn the guard red.
  No dependency change after D30a.

- [ ] **D30a — DEFERRED by D79, not v1.** The Fluent mechanism, the dependency, and the English catalog
  for the CLI surface. — [evidence](docs/history/m5.md#d30a)

- [ ] **D30b — BLOCKED by D30a.** the read-only path's messages — [brief](docs/tasks/D30b.md)

- [ ] **D30c — BLOCKED by D30b.** the write path's messages, the harness, the closing whole-tree gate, and the write
  path's locale evidence — [brief](docs/tasks/D30c.md)

- [ ] **D31 — DEFERRED by D79; the translations, in ordered groups.** One group is one PR. —
  [evidence](docs/history/m5.md#d31)

- [ ] **D31a — BLOCKED by D30c.** Lithuanian, the first translated catalog, as group 1 of D31; no group starts before
  D30c.
  Closes: the first non-English catalog and the first plural rule with three categories.
  Acceptance: the shared criteria of this milestone with `LANG=lt_LT.UTF-8`, both catalog gates passing for
  `locales/lt/main.ftl` with `lt` added to the shipped set the tests require, a `--list` sample in Lithuanian pasted
  in the PR, the container pair compared on exit code, container-local `/etc/apt` bytes, backup manifest bytes and
  harness tokens, with the printed text the only thing allowed to differ and the Lithuanian run required to print
  Lithuanian text — the first entry whose pair can discriminate; and the rename proof: renaming one English id makes
  `cargo test` fail naming that id as missing from `locales/lt/main.ftl`, then reverted, which is what makes
  `never derived from English text` falsifiable. [SPEC: docs/spec/localization.md]
- [ ] **D31b — BLOCKED by D30c.** Latvian and Estonian, group 2 of D31; taken after D31a. [SPEC:
  docs/spec/localization.md]
- [ ] **D31c — BLOCKED by D30c.** Swedish, Danish and Finnish, group 3 of D31; the EU official Nordic languages.
  [SPEC: docs/spec/localization.md]
- [ ] **D31d — BLOCKED by D30c.** Norwegian Bokmål and Icelandic, group 4 of D31; Nordic, not EU official languages.
  [SPEC: docs/spec/localization.md]
- [ ] **D31e — BLOCKED by D30c.** German, French and Dutch, group 5 of D31. [SPEC: docs/spec/localization.md]
- [ ] **D31f — BLOCKED by D30c.** Italian, Spanish and Portuguese, group 6 of D31. [SPEC: docs/spec/localization.md]
- [ ] **D31g — BLOCKED by D30c.** Greek and Bulgarian, group 7 of D31; the first non-Latin scripts shipped. [SPEC:
  docs/spec/localization.md]
- [ ] **D31h — BLOCKED by D30c.** Polish, Czech and Slovak, group 8 of D31. [SPEC: docs/spec/localization.md]
- [ ] **D31i — BLOCKED by D30c.** Hungarian, Romanian and Croatian, group 9 of D31. [SPEC: docs/spec/localization.md]
- [ ] **D31j — BLOCKED by D30c.** Slovenian and the closure check, group 10 of D31. [SPEC: docs/spec/localization.md]

- [ ] **D32 — BLOCKED by D30c; taken after D31j.** Simplified Chinese, the one catalog in a double-width script, and
  the script rule from docs/spec/localization.md — [brief](docs/tasks/D32.md)

- [ ] **D33 — BLOCKED by D31a and D32.** the localized manuals, and the answer to who owns `--help` —
  [brief](docs/tasks/D33.md)

- [ ] **D34 — DEFERRED by D79; human `chore:` PR; no delivery agent may make it.** three contracts contradict
  docs/spec/localization.md once D30a lands, and every delivery agent is closed to them — [brief](docs/tasks/D34.md)

## Backlog — opened by the SPEC restructure

**Status: OPEN** — none of these blocks the next milestone; each is a decision or a cleanup the restructure
  deliberately left in place.

- [x] D76a.
  Give every requirement a stable id, a `(planned)` marker where the code does not implement it, and one row per id in
  the `SPEC.md` index.
  Done in this branch: 117 ids across ten chapters, 32 marked planned and 6 marked partial. [SPEC:
  docs/spec/AGENTS.md]
- [x] **D76b — DONE.** The requirements speak normatively: `MUST`/`SHOULD`/`MAY` judged per id across all 117, with
  the index regenerated from those sentences. — [evidence](docs/history/backlog.md#d76b)
- [x] D77. Slim this file: it was 321 KB in 469 lines, 102 of them over 1000 characters. The evidence behind
  the 34 tasks that carried it moved to `docs/history/`, one file per milestone, and the change log to
`docs/history/change-log.md`; the ledger keeps each such task's claim and a link.
  The rest carried no evidence block, and the nine longest of their claims were D84's work.
- [x] D84. Trim the task claims that are still over 1000 characters in `ROADMAP.md`: the rationale of the nine
  finished tasks that carried one — D6e, D9a, D27, D40, D60–D63 and D76b — moved to `docs/history/` and their
  ledger lines keep the claim and a link, which took the file from 19 lines over 1000 characters to 10.
- [x] D85. Rule where an unstarted task's brief lives: seven entries (D53, D46, D30b, D30c, D32, D33, D34)
  were 1.4–3.0 KB inside `ROADMAP.md` and held no evidence, so they were neither archives nor claims.
  Each now has `docs/tasks/<id>.md`, kept verbatim with its status, and the ledger keeps one line: the claim, the
  state and a link. `docs/AGENTS.md` states the rule — forward-looking, deleted when the task lands, because
  `history/` holds what was done. The same pass reflowed the ledger to the style contract: it was 56 126 bytes in
  238 lines, three of them prose paragraphs over 1000 characters; it is 551 lines now, with no line over 120
  characters or over 120 bytes, and the text is word for word unchanged, checked the way D78 checked the record. The
  byte half is not pedantry: an em dash is three bytes, so two lines sat at 116 characters and 122 bytes, which a
  character measure calls short and a byte measure calls long. No byte total is stated, because this sentence sits
  inside the file it would describe.
  [docs/AGENTS.md]
- [x] D78. Apply the style contract in `docs/AGENTS.md` to `docs/threat-model.md`: one sentence per line and every
  prose line under 120 characters, with the rulings and the content untouched. It had 21 lines over 1000
  characters and now has none; 168 lines were rewrapped and the text is word for word the same, checked by
  comparing the whitespace-normalized file before and after, because a wrap that adds or drops a word is the one
  failure this kind of edit has. What is over 120 now is 20 table rows, which the contract exempts, and one line
  whose content is a single `rg` command in a code span, which breaking would corrupt.
  Two references into the file moved with the reflow and were re-pointed: B7 is `:366`, and the
  `sources.list.d` sentence D86 quotes is `:231`, and the record also had one reference to itself:
the terminal row said escaping is D13 (`:152`), which is `:366` now.
  The quotation in D86 was a quotation of the code,
  never of the record, and it now says so.
  [docs/AGENTS.md]
- [x] D79. Rule the localization question: **v1 prints English only**. `docs/spec/localization.md` stays the
  post-v1 design and its 13 `(planned)` rows stay as they are: the index already counts them, so a reader who
  wants to know what ships reads the state column. D30a–D34, D31a–D31j, D32, D33 and D21 are deferred with it and
  keep their state; none is v1. Evidence: every `LOC-*` row is `(planned)` except the English half of `LOC-14`
  (the manual page, which ships), `src/` has no `l10n` symbol, and `Cargo.toml` has one dependency. The Goal now
  says so, which is what turns an omission into a decision.
  [SPEC: docs/spec/localization.md]
- [x] D80. Rule the TUI and `--refresh` scope: **the TUI stays in v1**, which the Goal already says, so D13 keeps
  it and D14–D19 stay queued behind it; D21 is deferred with the
  translations (D79). `--refresh` is not in v1 (D81), and `CLI-3` and `CLI-7` keep it
  failing as an unknown option, which is the normative state today and not a gap.
  The part that could not wait is what this entry came with: B7 deferred terminal escaping to D13, and the
  exposure it names is a plain root CLI run printing a mirror URL or a rejected entry verbatim — today, with no
  TUI involved. It becomes **D89**, unblocked and independent: the four live paths are in `docs/threat-model.md`
  and the record now points the mitigation at D89 rather than at the TUI.
  [SPEC: docs/spec/tui.md, docs/spec/cli.md]
- [x] D81. Rule the mirror-list cache (D28): **out of v1**. D28 closes as not planned, `pipeline.md` keeps its
  `(planned)` row, and `--refresh` stays an unknown option exactly as `CLI-3` and `CLI-7` require.
  Evidence: the cache buys nothing measurable today — one list page and one `InRelease` per run — while it would
  be the first thing the tool writes outside `/etc/apt` and `/var/backups/apt-scout`: a file that outlives the
  run, is read by a later run, and can be poisoned by whoever can write it. A new trust surface for a latency
  nobody complained about is the wrong trade for v1.
  [SPEC: docs/spec/pipeline.md]
- [x] D82. Re-point the stale `path:line` citations in `docs/threat-model.md` that predated the restructure:
16 distinct stale citations, 22 occurrences at 13 places in the record, re-pointed and each verified by the symbol
  on the cited
  line (`main.rs:861–862` is the `NO_COLOR` read, `:2510–2515` the non-root exit 3, `:3446–3448` the harness
  dispatch). Two sentences moved with their citations: the terminal row now names the newline-free `print!`
  prompt path (`main.rs:553`) beside `println!`, and the S16 bullet says that W10 and W11 add
  tmpfs-only mounts (`:123`, `:455`). Every citation in the file now resolves inside its target.
- [x] D86. Audit the `path:line` citations in `docs/threat-model.md` that the triage flags, against the code, not
  just their bounds: `scripts/citation-triage.py` compares the words of five characters or more in the unit that holds
  a citation with those of the cited line and the five either side of it, and prints the citations that share none.
  Every one was read here against the line it names, and all of them are correct: the flags are word mismatches, not
  drift. The record's sentence about `sources.list.d` (`docs/threat-model.md:231`) cites
  `src/sources.rs:653`, where the refusal is the string "not a directory; symlinks are not followed" inside an
  `issue: Some(...)` field, so the words that matter are not the words the triage compares.
  The unit matters. Reading one physical line at a time flagged 22 citations, and D78's reflow raised that to 65
  with no citation changed, because a wrapped fragment shares fewer words with the code than its sentence does.
  Reading whole paragraphs swung the other way to 12, because consecutive bullets have no blank line between them
  and a ten-bullet list became one unit: a coarser unit hides drift instead of inventing it. A unit is now a bullet,
  a numbered item, a table row or a paragraph, with its indented continuation lines, and it prints 22 over 19
  distinct targets on both the reflowed file and `develop`'s copy. All 22 are the citations audited by hand here.
  The printed location is the line that carries the citation, not the line that opens its unit, so a reader lands
  on the sentence rather than on the bullet's first line.
  Two of the four citations D87 had moved were in the first list before this branch fixed them, and two were not, so
  the triage catches some drift, not all of it, and a reader stays the instrument.
  A stricter check was built and abandoned: for each citation attached to a backticked name, require that name within
  five lines of the cited line. It flagged 5 of 32 named citations and all five were false, because an environment
  variable never appears in the code that reads it and a sentence listing two functions pairs the second name with
  the first citation. A check whose flags are mostly wrong teaches the reader to ignore it, so it was deleted rather
  than tuned.
  [docs/AGENTS.md]
- [x] D88. Decide whether `scripts/citation-triage.py` runs in CI, and where. It runs in the weekly
  `source-contract.yml` advisory, beside `check-sources.sh`: the same workflow and the same contract of
  advisory-never-required, with its output in the step summary rather than a second kind of issue.
  It is not a required check because the flag set is a triage: a required step would fail today on a list that is
  mostly correct citations, and a check that fails for the wrong reason gets switched off. What it buys is a weekly
  list written down, so drift is seen before an audit finds it. It catches two of the three places D87 moved and not
  the third, so a reader stays the instrument; D86 records that measurement.
  [.github/workflows/source-contract.yml]
- [x] D87.
  Audit the one process a root run spawns for its environment: `src/main.rs` ran `/usr/bin/dpkg --print-architecture`
  with the inherited environment, so a variable a user could set before `sudo` reached a process running as root,
  and the vector was the dynamic loader (`LD_PRELOAD`, `LD_AUDIT`, `LD_LIBRARY_PATH`): code execution in the root
  process. It was a compliance defect rather than a new rule, because the record already required the run to pass a
constructed environment to any child.
  The child is now built by `cleared_command` (`src/main.rs:2738`), which clears the
  environment and closes stdin the way the verifier does, and `a_cleared_command_starts_with_no_environment` runs
  `/usr/bin/env` through the same builder, asserting the output is exactly the one variable it sets: it fails when
  `.env_clear()` is removed, which was checked by removing it. A failing child, and a child that prints
  nothing, are now reported with their own message, status and a next step instead of arriving as an empty
  architecture that the ports arm would then describe as an unsupported CPU; neither failure path has a test, so
  that branch is held by review too. `docs/spec/platform.md` PLAT-3 states the
  requirement; the record says the children are cleared and that the run's own loader cannot be, since it runs before
  any line of the program. The test covers the builder, not the use site: nothing pins that `architecture()`
  calls `dpkg_command()`, and a second spawn written later would not be caught by it, so that wiring is held by
  review rather than by a gate, and the stdin half of the requirement is untested for the same reason: a builder that
  cleared the environment and inherited stdin would pass.
  [docs/spec/platform.md, docs/threat-model.md]
- [ ] **D83 — READY; one of the ten chapters is moved.** Move the rationale and the measurement
  evidence out of the requirement bodies in `docs/spec/`, because `docs/AGENTS.md` sends `why` to an ADR and evidence
  to `ROADMAP.md`. D76b left both in place deliberately, because three measured claims are what the
  parsers are tested against. `pipeline.md` is done: its measurements, with their dates, are
  [ADR-0005](docs/decisions/0005-mirror-list-and-freshness-measurements.md), and each requirement that rested on
  one cites that record or the decision that owns the rule.
  Remaining, eight chapters: `apply.md` and `output.md`, then `cli.md`, `errors.md`,
  `localization.md`, `testing.md`, `non-functional.md` and `platform.md`.
  `tui.md` is the tenth chapter and carries no rationale or dated observation, so it needs no move.
  Acceptance: every `because`, `so that`, `Verified from`, `Measured` and dated observation inside a requirement
  is either cited as `ADR-####` or moved to a ledger entry, and no requirement is left without the fact it needs to
  be testable. No behavior change. [docs/AGENTS.md, SPEC: docs/spec/AGENTS.md]

## Change log

Historical log: [docs/history/change-log.md](docs/history/change-log.md).
