# DOX framework

- DOX is highly performant AGENTS.md hierarchy installed here
- Agent must follow DOX instructions across any edits

## Core Contract

- AGENTS.md files are binding work contracts for their subtrees
- Work products, source materials, instructions, records, assets, and durable docs must stay understandable from the nearest applicable AGENTS.md plus every parent AGENTS.md above it

## Read Before Editing

1. Read the root AGENTS.md
2. Identify every file or folder you expect to touch
3. Walk from the repository root to each target path
4. Read every AGENTS.md found along each route
5. If a parent AGENTS.md lists a child AGENTS.md whose scope contains the path, read that child and continue from there
6. Use the nearest AGENTS.md as the local contract and parent docs for repo-wide rules
7. If docs conflict, the closer doc controls local work details, but no child doc may weaken DOX

Do not rely on memory. Re-read the applicable DOX chain in the current session before editing.

## Update After Editing

Every meaningful change requires a DOX pass before the task is done.

Update the closest owning AGENTS.md when a change affects:

- purpose, scope, ownership, or responsibilities
- durable structure, contracts, workflows, or operating rules
- required inputs, outputs, permissions, constraints, side effects, or artifacts
- user preferences about behavior, communication, process, organization, or quality
- AGENTS.md creation, deletion, move, rename, or index contents

Update parent docs when parent-level structure, ownership, workflow, or child index changes. Update child docs when parent changes alter local rules. Remove stale or contradictory text immediately. Small edits that do not change behavior or contracts may leave docs unchanged, but the DOX pass still must happen.

## Hierarchy

- Root AGENTS.md is the DOX rail: project-wide instructions, global preferences, durable workflow rules, and the top-level Child DOX Index
- Child AGENTS.md files own domain-specific instructions and their own Child DOX Index
- Each parent explains what its direct children cover and what stays owned by the parent
- The closer a doc is to the work, the more specific and practical it must be

## Child Doc Shape

- Create a child AGENTS.md when a folder becomes a durable boundary with its own purpose, rules, responsibilities, workflow, materials, or quality standards
- Work Guidance must reflect the current standards of the project or user instructions; if there are no specific standards or instructions yet, leave it empty
- Verification must reflect an existing check; if no verification framework exists yet, leave it empty and update it when one exists

Default section order:
- Purpose
- Ownership
- Local Contracts
- Work Guidance
- Verification
- Child DOX Index

## Style

- Keep docs concise, current, and operational
- Document stable contracts, not diary entries
- Put broad rules in parent docs and concrete details in child docs
- Prefer direct bullets with explicit names
- Do not duplicate rules across many files unless each scope needs a local version
- Delete stale notes instead of explaining history
- Trim obvious statements, repeated rules, misplaced detail, and warnings for risks that no longer exist

## Closeout

1. Re-check changed paths against the DOX chain
2. Update nearest owning docs and any affected parents or children
3. Refresh every affected Child DOX Index
4. Remove stale or contradictory text
5. Run existing verification when relevant
6. Report any docs intentionally left unchanged and why

## User Preferences

When the user requests a durable behavior change, record it here or in the relevant child AGENTS.md

- Chat language: agent talks to the user in Lithuanian; project artifacts stay English
- Language: all project artifacts are English only: AGENTS.md, skills, rules, specs, docs, code comments, commit messages, PR text, CLI output, error messages
- Spec-driven development (SDD) allowed: keep a single English `SPEC.md` at the repo root if behavior needs a spec; code follows the spec
- Delivery team lives in the repo, not in the global config: `.pi/agents/` holds `apt-scout-lead` (owns the ROADMAP loop and merges), `apt-scout-architect` (owns SPEC/ROADMAP/plans), `apt-scout-coder` (the only code writer), `apt-scout-reviewer` (independent code review) and `apt-scout-security` (trust-boundary audit). Edit them here. Delegation must pass `agentScope: project` (or `both`); the runtime default is already `both`, which loads the user-level agents and this folder, so `project` is what keeps a same-named user agent from shadowing the team. The lead is the only agent that merges into `develop`, and only through a PR with green checks plus a review and security verdict. Model choice is not a project rule: the machine-level rule in the global agent config decides it.
- Long-running commands: never leave an open-ended blocking command. Bound it and log it (`timeout 420 bash scripts/docker-test.sh > /tmp/docker.log 2>&1`), then poll the log with short calls. Ceiling per command: 7 min for `scripts/docker-test.sh`, 5 min for cargo/deny; a bound hit is a result to report, not a reason to retry bigger.
- Shared cargo target: all agent work runs with `CARGO_TARGET_DIR=/tmp/apt-scout-target` so worktrees reuse incremental builds. `scripts/docker-test.sh` is a pre-merge runtime gate, not a per-task one; docs-only tasks run no cargo.
- Standing merge authority: green checks plus a review verdict and a security verdict on the exact head are the whole condition for merging into `develop`. The lead merges without asking; red checks or an open P0/P1 are fixed until green, not escalated. Only a ruling change, unsettled product behavior, credentials or the privilege boundary go to the user.
- Quality over speed: do not batch unrelated roadmap tasks into one PR to save wall time, and do not trim a gate (full Docker matrix, review, security audit, Gate 2) for speed. One task, one coherent PR. Speed comes only from removing fixed cost — shared build cache, parallel read-only reviews, no wall-clock polling — never from weaker verification.

## Project

- `apt-scout`: Rust binary crate (edition 2024); one dependency, `ureq` with rustls (no OpenSSL; `webpki-roots` brings the CDLA-Permissive-2.0 license, allowed in `deny.toml`)
- Purpose: standalone Rust CLI (no Python/runtime deps) doing what `nala fetch` does, for Ubuntu and derivatives (Pop!_OS, Mint) without installing nala
- Current read-only flow: detect server country and AS (optional; country detection failure asks for `--country`) -> fetch Launchpad mirror list (https://launchpad.net/ubuntu/+archivemirrors) -> filter by country -> probe freshness/speed -> rank. Apply/source rewriting is not implemented yet.
- Constraints: backup apt sources before write; root needed only for write; prefer rustls over openssl; static/musl-friendly
- Reference: https://github.com/volitank/nala (`nala fetch`)
- Name: `apt-scout` (final)
- `Cargo.toml` / `Cargo.lock`: crate manifest and lockfile
- `SPEC.md`: source of truth for behavior (scope, CLI, pipeline, apply rules, exit codes); update it before changing behavior. `ROADMAP.md`: execution ledger, task states, dependencies, merged PRs, verification evidence, and next task.
- `GLOSSARY.md`: project vocabulary only, no implementation detail
- `src/main.rs`: CLI and pipeline (`--country`, `--no-geo`, `--top`, `--https-only`, `--list`, read-only for now; optional country/AS detection and Team Cymru ranking; Launchpad then `mirrors.ubuntu.com` fallback; reference-unreachable fallback to newest sane mirror date); `src/distro.rs`: os-release detection (Ubuntu family, codename; last duplicate assignment wins); `src/mirrors.rs`: Launchpad HTML and `mirrors.ubuntu.com` list parsers and candidate URL validation (pure, tested on `tests/fixtures/`); `src/countries.rs`: ISO code to Launchpad country name (test checks it against the full Launchpad page fixture); `src/fresh.rs`: InRelease `Date` parsing, lag, `MAX_LAG_SECS`, ranking (pure); `src/probe.rs`: per-mirror probe through the `Fetch` trait, including full `InRelease` signature status; `src/verify.rs`: fixed-path `/usr/bin/gpgv` + Ubuntu archive keyring verification, no shell/PATH lookup; `src/http.rs`: HTTP network access module (`ureq` + rustls; resolver drops private addresses; proxies ignored; manual redirects, every hop validated; one 10 s budget per call; size caps; `Canned` fake for tests); `scripts/docker-test.sh`: container tests
- `src/geo.rs`: optional server country and AS from third-party sources, every failure leaving the answer unknown and the run continuing; parsers pure, tested on anonymised fixtures, answers validated. `src/cymru.rs`: Team Cymru IP-to-ASN lookup over plain DNS TXT (packet builder and answer parser pure and tested; only `lookup` touches the network). `src/sources.rs`: read-only apt source inventory and parsers (legacy and deb822, symlinks refused); `rewrite_source` returns a byte-preserving rewrite plan and performs no filesystem IO
- `src/digest.rs`: std-only SHA-256 and lowercase hex (pure, no IO). It describes a backup file by content for the manifest, and from D9b the writer compares bytes directly; no hash ever authorizes a write
- `src/plan.rs`: the closed target enumeration and the `Plan`/`PlannedWrite` validation every write passes. A target can only be `sources.list` or a validated entry under `sources.list.d`, so no path comes from input; the plan carries the selected mirror, its verified proof and each write's bytes with the digest that binds them
- `src/manifest.rs`: `render` and `parse` for the backup manifest. `parse` is the format's definition, fails closed on anything `render` would not have written — order, counts, size caps, root ownership — and nothing here touches a disk
- `.pi/README.md`: how the delivery team is launched (the one `subagent(...)` call that runs the loop, and the per-step form for one task), what a machine needs, and what `scripts/agent-doctor.sh` checks and does not check. `scripts/agent-doctor.sh`: the preflight for a machine that is about to run the team — one PASS/FAIL/WARN line per requirement with the fix for each failure, exit non-zero while something required is missing; run it in the clone Pi will be launched in
- `target/`: build output, never edit
- Repo: public GitHub repository `apt-scout`; license MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`)
- Branches: `develop` collects work, `main` = last release. Feature branches `feat/*`, `fix/*`, `chore/*` -> PR into `develop` (squash). Never push directly to `main` or `develop`.
- Release flow (no manual tagging): run workflow `Prepare release` with version X.Y.Z -> PR `release/vX.Y.Z` into `develop` (bumps `Cargo.toml`, writes `CHANGELOG.md` from Conventional Commits via `.github/scripts/changelog.sh`) -> merge -> workflow `Release PR` opens `develop` -> `main` PR (merge commit) -> merge -> workflow `Release` builds musl binary, `.tar.gz`, `.deb`, `SHA256SUMS`, tags `vX.Y.Z`, publishes GitHub Release
- Documentation rule: update docs in the same PR as any change to behavior, structure, workflow or settings. Order: nearest `AGENTS.md` (and affected parents/children), `SPEC.md` (update it before the code), `GLOSSARY.md`, `README.md`, code comments, and the setup runbook (`rust-cli-setup-runbook.md`, kept outside this repo, next to it in the parent directory) when repo layout, CI, rulesets or release flow change. `CHANGELOG.md` is generated from commit subjects, never hand-edited. If nothing needs updating, write `Docs: no change needed, because ...` in the PR. This is not enforced by CI; the PR template checklist and the reviewer enforce it, and a behavior change without a docs update is incomplete
- Spec-driven delivery cycle: use `ROADMAP.md` as the durable task ledger and `SPEC.md` as the behavior contract. Start each task from the next READY roadmap item; update SPEC first when behavior is missing or wrong. After each merged PR, immediately mark the item DONE with PR and verification evidence, then start the next unblocked item. Keep task states and dependencies current; discovered work becomes a roadmap item before it is forgotten.
- Parallel agents: use only for independent bounded work in isolated branches/worktrees; one writer per worktree and no concurrent edits to shared files. Use a reviewer on the exact PR diff before merge; fix or answer all findings, wait for required CI, then merge. Never push directly to `develop` or `main`.
- Milestone gates: at every milestone boundary, ask Oracle to audit the original goal against SPEC, delivered implementation, tests, and real-system evidence. Add accepted missing requirements to SPEC, then update ROADMAP before continuing. TUI milestone must include laptop terminal launch for user visual inspection and local Ubuntu Docker-emulated runtime tests; do not claim visual acceptance until the user confirms.
- Privilege-security gates: before implementing root/privileged file writes, require an Oracle threat-model/design audit and an independent security-focused review; record findings and closure in ROADMAP. Never add or assume `sudo` self-reexec is safe. Network, parsing, rewrite planning, and confirmation stay unprivileged; root code must not execute shell commands, make network requests, or trust user-controlled PATH/environment. Test privileged operations only in disposable Ubuntu containers; never use sudo or mutate host apt files.
- Code review (mandatory before merging any PR): dispatch `apt-scout-reviewer` on `origin/develop...HEAD` before the PR exists, then on the final remote diff (`gh pr diff <N>`) after every fix; check docs per the Documentation rule, and address or answer every finding in the PR — the ruleset requires thread resolution, so an unanswered thread blocks the merge. Use Oracle for design or spec questions.
- `.github/`: `workflows/{ci,pr-title,prepare-release,release-pr,release,source-contract}.yml`, `scripts/changelog.sh`, `scripts/check-sources.sh`, `CODEOWNERS`, `dependabot.yml` (weekly, targets `develop`), PR and issue templates. Required status checks on `main`/`develop`: `check` (fmt, clippy -D warnings, test, release build), `pr-title` (Conventional Commit title). Also required: `deny` (cargo-deny: advisories, licenses, no OpenSSL), `musl-build` (static binary), `docs-and-conventions` (English-only guard via `rg`)
- CI scope: `.rs`, Cargo manifests/lockfiles, toolchain/rustfmt/clippy/deny configuration, `.cargo/`, `.github/workflows/`, and `.github/scripts/` run full CI. Other changes skip `check`, `deny`, and `musl-build` at job level (GitHub accepts skipped jobs as passing required statuses). Documentation and PR-title checks still run. Manual dispatch and initial pushes run full CI. Diff detection errors fail the `changes` job and default downstream jobs to full CI; never use workflow-level path filters for required checks.
- Musl builds (CI and releases) use `cross` with its prebuilt target container, installed via `taiki-e/install-action`; do not install `musl-tools` through runner apt mirrors. Docker and access to the cross container registry are required on the runner.
- Testing: unit tests on fixtures; integration tests in Docker on Ubuntu 20.04, 22.04, 24.04, 26.04 through `scripts/docker-test.sh` (builds a static amd64 binary in the prebuilt `ghcr.io/cross-rs` musl image, runs it in real Ubuntu images; offline cases use `--network none`, and one live case per release uses the real Launchpad and mirrors, so it needs internet). Docker tests run only on the developer's machine, never in CI; run them before any PR that changes `.rs` or build configuration and say so in the PR. CI only builds and runs static checks; see `SPEC.md` Testing. Live-network tests are manual only
- `source-contract.yml` (weekly, advisory, never required): runs `scripts/check-sources.sh` against the live data sources and opens one issue after two consecutive failures. Run `scripts/check-sources.sh` locally to verify sources before changing a parser
- Toolchain files: `rust-toolchain.toml` (stable + rustfmt + clippy), `rustfmt.toml`, `deny.toml`
- Use `rg` (ripgrep), not `grep`, for searching
- Git guard: some developer machines enforce a repository-kind setting and a matching git identity; if a commit is rejected, follow the fix printed by the guard. Do not record personal identities in project documentation.
- Rust rules: no `unwrap()`/`expect()` outside tests; errors carry context (see `SPEC.md` Errors and self-healing); `unsafe` only with a `// SAFETY:` comment; std over new dependencies; `rustls`, never OpenSSL; pure logic separate from IO; tests use fixtures, never live network
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `feat!:` breaking); the changelog is generated from subjects
- Repo files are authoritative: `AGENTS.md` for workflow rules, `.github/workflows/ci.yml` for required checks, `.github/workflows/pr-title.yml` for the Conventional Commit PR title, `ROADMAP.md` for task order and status.
- Verification: `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo deny check`, `scripts/docker-test.sh` (needs Docker). Run the full set before PRs that change Rust/build behavior and record results in ROADMAP.

## Child DOX Index

- `.pi/agents/AGENTS.md` — contract for the delivery team's five agent files: who may edit them and the read-only convention they share. Its sibling `.pi/README.md` covers the same boundary from the operator's side — launching the loop, what a machine needs, and what `scripts/agent-doctor.sh` checks — and states no rules of its own.
- `docs/AGENTS.md` — contract for the durable decision records: who owns `threat-model.md`, that a ruling or an open design gap moves only with human approval, and the checks that keep its delta ledger and its citations honest.

No `src/` child doc: module behavior lives in the Project bullets above, and no source folder carries rules of its own yet; add a child doc when one does — for rules, not for module descriptions.
