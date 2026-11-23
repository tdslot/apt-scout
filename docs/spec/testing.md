# Testing

- `TEST-1` An assertion MUST compare a value with an expectation that does not derive from the same expression
  as the value.
  A test whose two sides are computed one from the other, or whose expected side cannot differ from
  the produced one in the environment it runs in, passes whatever the code does: it is a defect in
  the test, not a passing gate.
  A test written or changed is therefore shown to fail when the code it covers is broken — mutate
  that code, watch the test report the failure, restore it — or the PR states why no such mutation
  can exist.
- `TEST-2` Unit tests (`#[cfg(test)]`) MUST cover pure logic on fixtures: `os-release`, Launchpad HTML,
  `mirrors.txt`, `InRelease` dates, freshness, ranking, read-only apt source inventory/parsers, and
  D8 source rewrite output/round trips.
  Saved fixtures come from real responses (the Launchpad page is ~316 KB; keep a trimmed copy with
  at least two countries and every status value seen).
- `TEST-3` Docker integration tests MUST run inside disposable real Ubuntu images, and MUST include
  offline cases and live-network smoke cases.
  The live cases MUST be manual and MUST never be part of CI; they need internet.
  The matrix MUST cover every supported release; format measured on 2026-11-11:

  | Release | Codename | Sources format |
  | --- | --- | --- |
  | 20.04 | focal | legacy `/etc/apt/sources.list` (10 `deb` lines) |
  | 22.04 | jammy | legacy `/etc/apt/sources.list` |
  | 24.04 | noble | deb822 `/etc/apt/sources.list.d/ubuntu.sources` |
  | 26.04 | resolute | deb822 `/etc/apt/sources.list.d/ubuntu.sources` |

  Fixtures for verification live in `tests/fixtures/signed/`: the real `noble-updates` `InRelease`,
a tampered copy of it, and the Ubuntu archive keyring.
A forged body is built in the test from the signed fixture by prefixing a `Date:` line, so the
regression proves all three facts at once: the prefix verifies under `gpgv`, the signed-region parse
yields no date for it, and read-only ranking still uses the displayed date.
Unit tests that need a real `gpgv` and keyring skip when `/usr/bin/gpgv` or the archive keyring is
absent (developer hosts), and the Docker cases are the end-to-end proof of the positive path.
Releases whose reference `InRelease` returns 404 are handled by the exit-4 rule in `pipeline.md` step 8.
`focal-updates` was still receiving updates on 2026-11-11 (`Date` 2026-11-11), so freshness stays
meaningful for 20.04; keep a focal fixture. `i386` is in scope but covered by fixtures only (no i386
image).
The `amd64` images are used (`--platform linux/amd64`); on Apple Silicon they run under emulation,
slower but equivalent.
Current Docker tests cover distro detection, read-only inventory/early stop, live mirror ranking,
the missing-`gpgv` case (every signature reads `unavailable (<reason>)` while ranking and the exit
code stay usable), one live case that requires a real `verified` reference and at least one
`verified` mirror row after installing the two `.deb` dependencies, because a base image is not a
package install, and the restore cases W27–W33, which reach the flag the user types against a backup
a real write made, including the interactive confirmation answered both ways.
Later source-write milestones MUST test rewrite, backup, atomic write, `--restore` and error
messages inside disposable containers with isolated `/etc/apt`; the restore cases are W27–W33 in
`scripts/docker-test.sh` (a positive restore after a real write, a second restore refused, a
non-root run that opens no backup, a tampered copy, a target replaced behind the backup's back, and
the confirmation answered `y` and `n` on standard input).
Network answers in automated tests are otherwise injected from fixtures.
- `TEST-4` (planned) Localization MUST be tested in two layers, and a catalog MUST ship only when both
  pass.
  In-process, over the files the binary carries: every `locales/*/main.ftl` parses (a failure names
  the file and position), the compiled catalogs load, the shipped set the binary reports equals the
  set of locale directories, every English message id exists in every shipped locale with the
  missing ids listed on failure, placeholder parity per message, a `#` comment on every message,
  every plural category the language's CLDR rule can return, the mapping table (`lt_LT.UTF-8` to
  `lt`, `de_AT.UTF-8` to `de`, `zh_CN.UTF-8` to `zh-Hans`, `zh_TW.UTF-8` to English, `zh_Hans_*` and
  `zh_Hant_*` by script, `C`, `POSIX`, empty and unset to English, an unshipped language to
  English), the width function against its fixture table, and the literal gate that refuses a
  printed sentence outside `src/l10n.rs`.
  The completeness gate's discriminating power over English-tied ids is shown in the same layer:
  from the first translated catalog on, renaming one English id MUST fail the completeness gate for
  every shipped locale other than English, naming the id as missing.
  In a container: the read-only CLI and a root write run each executed twice against the same
  fixture image, once under `LANG=C` and once under a shipped locale, compared on exactly four
  things — the exit code, the bytes of the container-local `/etc/apt` tree, the backup manifest's
  bytes for the write pair, and the harness tokens — with the printed display text the only thing
  allowed to differ.
  The root pair is the write path's evidence; the read-only pair says nothing about it.
  A pair whose two runs print the same text is two copies of one run — with English as the only
  shipped catalog it passes whatever the write path reads — so the pair discriminates only from the
  first translated catalog on, whose own entry carries the same four comparisons and requires the
  translated run to print that locale's text.
  Neither pair is claimed to cover a path its fixture does not reach.
- `TEST-5` One script (`scripts/docker-test.sh`) MUST run the container tests on the developer's machine
  only, and CI MUST NOT run Docker tests (CI builds and runs static checks).
  The write cases run against the container's own `/etc/apt`, with no path override of any kind:
  there is no runtime override, because the writer reads no environment variable, and, once those
  cases land, the write entry point they use MUST exist only in a build compiled with the test-only
  `apt_scout_write_harness` cfg, which `scripts/docker-test.sh` MUST set with a build flag passed to
  that crate's own compilation when it builds that binary, and a release build MUST NOT set it.
  The toolchain inside its build container comes from the published `rustup-init` binary checked
  against the `.sha256` served from the same origin, never from a script piped into a shell: the
  container has the checkout mounted read-write, so the content it executes MUST be verifiable.
  CI compiles that cfg build as well, so the file cannot rot unseen: the `check` job runs clippy a
  second time with `RUSTFLAGS=--cfg apt_scout_write_harness`, and a deliberate syntax error in
  `src/harness.rs` is reported by that pass and by no other.
  Run it before opening a PR that changes `.rs` files or build configuration, and state the result
  in the PR.
  TUI milestone additionally requires a laptop launch against safe fixture/mock data for user visual
  inspection, followed by runtime checks in local Ubuntu Docker emulation; visual acceptance
  requires explicit user confirmation.
- `TEST-6` Pop!_OS MUST be tested in an Ubuntu 22.04 container with the real `pop-default-settings` package
  downloaded from `apt.pop-os.org` (pinned by file checksum in the test fixtures, downloaded once
  and cached; the download is a test setup step, not a test of the live service): its `os-release`
  and the deb822 `system.sources` template from its `postinst` are the fixture.
  Linux Mint MUST be covered by its documented `os-release` (`ID=linuxmint`, `VERSION_CODENAME=wilma`,
  `UBUNTU_CODENAME=noble`) in a container.
  Neither is a full installation, only the files this tool reads and writes.
- `TEST-7` Live-network smoke tests (real Launchpad, geo sources) MUST be manual and MUST never be part
  of CI, because they are rate limited and flaky.
- `TEST-8` (planned) Man-page verification MUST check `man/apt-scout.1` formatting (for example with
  `mandoc -Tlint`) and MUST confirm `man apt-scout` renders in a disposable Ubuntu container.
  Every example MUST match `--help` and the implemented CLI; a planned flag MUST NOT be documented as
  available.
- `TEST-9` Every call site that rewrites a control character MUST be pinned by a test, and the count of
  those sites MUST be asserted by a test rather than stated in prose, so a new site fails the suite until it is
  either covered or recorded as carrying no text the run did not author.
- `TEST-10` A container case MUST prove that a control character in a mirror URL reaches the terminal as
  visible text: a `--cfg apt_scout_write_harness` build MUST serve a list whose URL carries one, print what the run
  says about it, and the case MUST fail on a raw control byte in that output. `mirrors::validate_url` rejects such
  a URL before it is probed, so the case proves the escaping at the print site and not the rejection.
