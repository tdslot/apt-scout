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

## Project

- `apt-scout`: Rust binary crate (edition 2024); one dependency, `ureq` with rustls (no OpenSSL; `webpki-roots` brings the CDLA-Permissive-2.0 license, allowed in `deny.toml`)
- Purpose: standalone Rust CLI (no Python/runtime deps) doing what `nala fetch` does, for Ubuntu and derivatives (Pop!_OS, Mint) without installing nala
- Flow: detect server country and AS (optional; never fatal) -> fetch Launchpad mirror list (https://launchpad.net/ubuntu/+archivemirrors) -> filter by country -> probe latency/speed -> rewrite apt sources (`/etc/apt/sources.list` or `/etc/apt/sources.list.d/*.sources`) to fastest mirror
- Constraints: backup apt sources before write; root needed only for write; prefer rustls over openssl; static/musl-friendly
- Reference: https://github.com/volitank/nala (`nala fetch`)
- Name: `apt-scout` (final)
- `Cargo.toml` / `Cargo.lock`: crate manifest and lockfile
- `SPEC.md`: source of truth for behavior (scope, CLI, pipeline, apply rules, exit codes); update it before changing behavior
- `GLOSSARY.md`: project vocabulary only, no implementation detail
- `src/main.rs`: CLI and pipeline (`--country`, `--top`, `--https-only`, `--list`, read-only for now); `src/distro.rs`: os-release detection (Ubuntu family, codename; last duplicate assignment wins); `src/mirrors.rs`: Launchpad HTML and `mirrors.ubuntu.com` list parsers and candidate URL validation (pure, tested on `tests/fixtures/`); `src/countries.rs`: ISO code to Launchpad country name (test checks it against the full Launchpad page fixture); `src/fresh.rs`: InRelease `Date` parsing, lag, `MAX_LAG_SECS`, ranking (pure); `src/probe.rs`: per-mirror probe through the `Fetch` trait; `src/http.rs`: the only module that uses the network (`ureq` + rustls, manual redirects with validation, size caps, `Canned` fake for tests); `scripts/docker-test.sh`: container tests
- `target/`: build output, never edit
- Repo: https://github.com/tdslot/apt-scout (public, `tdslot`); license MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`)
- Branches: `develop` collects work, `main` = last release. Feature branches `feat/*`, `fix/*`, `chore/*` -> PR into `develop` (squash). Never push directly to `main` or `develop`.
- Release flow (no manual tagging): run workflow `Prepare release` with version X.Y.Z -> PR `release/vX.Y.Z` into `develop` (bumps `Cargo.toml`, writes `CHANGELOG.md` from Conventional Commits via `.github/scripts/changelog.sh`) -> merge -> workflow `Release PR` opens `develop` -> `main` PR (merge commit) -> merge -> workflow `Release` builds musl binary, `.tar.gz`, `.deb`, `SHA256SUMS`, tags `vX.Y.Z`, publishes GitHub Release
- Documentation rule: update docs in the same PR as any change to behavior, structure, workflow or settings. Order: nearest `AGENTS.md` (and affected parents/children), `SPEC.md` (update it before the code), `GLOSSARY.md`, `README.md`, code comments, and the setup runbook (`rust-cli-setup-runbook.md`, kept outside this repo, next to it in the parent directory) when repo layout, CI, rulesets or release flow change. `CHANGELOG.md` is generated from commit subjects, never hand-edited. If nothing needs updating, write `Docs: no change needed, because ...` in the PR. This is not enforced by CI; the PR template checklist and the reviewer enforce it, and a behavior change without a docs update is incomplete
- Code review (mandatory before merging any PR): run the `reviewer` subagent on the PR diff (`gh pr diff <N>`), check docs per the Documentation rule, and address or answer every finding in the PR; use `oracle` for design or spec questions.
- `.github/`: `workflows/{ci,pr-title,prepare-release,release-pr,release,source-contract}.yml`, `scripts/changelog.sh`, `scripts/check-sources.sh`, `CODEOWNERS`, `dependabot.yml` (weekly, targets `develop`), PR and issue templates. Required status checks on `main`/`develop`: `check` (fmt, clippy -D warnings, test, release build), `pr-title` (Conventional Commit title). Also required: `deny` (cargo-deny: advisories, licenses, no OpenSSL), `musl-build` (static binary), `docs-and-conventions` (English-only guard via `rg`)
- CI scope: `.rs`, Cargo manifests/lockfiles, toolchain/rustfmt/clippy/deny configuration, `.cargo/`, `.github/workflows/`, and `.github/scripts/` run full CI. Other changes skip `check`, `deny`, and `musl-build` at job level (GitHub accepts skipped jobs as passing required statuses). Documentation and PR-title checks still run. Manual dispatch and initial pushes run full CI. Diff detection errors fail the `changes` job and default downstream jobs to full CI; never use workflow-level path filters for required checks.
- Musl builds (CI and releases) use `cross` with its prebuilt target container, installed via `taiki-e/install-action`; do not install `musl-tools` through runner apt mirrors. Docker and access to the cross container registry are required on the runner.
- Testing: unit tests on fixtures; integration tests in Docker on Ubuntu 20.04, 22.04, 24.04, 26.04 through `scripts/docker-test.sh` (builds a static amd64 binary in the prebuilt `ghcr.io/cross-rs` musl image, runs it in real Ubuntu images; offline cases use `--network none`, and one live case per release uses the real Launchpad and mirrors, so it needs internet). Docker tests run only on the developer's machine, never in CI; run them before any PR that changes `.rs` or build configuration and say so in the PR. CI only builds and runs static checks; see `SPEC.md` Testing. Live-network tests are manual only
- `source-contract.yml` (weekly, advisory, never required): runs `scripts/check-sources.sh` against the live data sources and opens one issue after two consecutive failures. Run `scripts/check-sources.sh` locally to verify sources before changing a parser
- Toolchain files: `rust-toolchain.toml` (stable + rustfmt + clippy), `rustfmt.toml`, `deny.toml`
- Use `rg` (ripgrep), not `grep`, for searching
- Git guard: this repo needs `git config nep.repo-kind personal` and identity `tdslot <tdslot@gmail.com>` (`git-id personal`)
- Rust rules: no `unwrap()`/`expect()` outside tests; errors carry context (see `SPEC.md` Errors and self-healing); `unsafe` only with a `// SAFETY:` comment; std over new dependencies; `rustls`, never OpenSSL; pure logic separate from IO; tests use fixtures, never live network
- Commits: Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `feat!:` breaking); the changelog is generated from subjects
- Agent-local skill `apt-scout-dev-workflow` (outside the repo, in the agent's project memory) mirrors these rules; the repo files are authoritative
- Verification: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `scripts/docker-test.sh` (needs Docker)

## Child DOX Index

None. Project is a single small crate (one source file); no folder is a durable boundary yet. Add child AGENTS.md (e.g. `src/AGENTS.md`) when modules appear.
