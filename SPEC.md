# apt-scout SPEC

Standalone Rust CLI.
Picks the fastest, fresh, nearby Ubuntu archive mirror and rewrites apt sources.
Equivalent of `nala fetch` without installing nala.
No Python or other runtime.

This file is the map: the requirement text lives in `docs/spec/`, one file per area.

## How to read

- `docs/spec/` is normative: what apt-scout MUST, SHOULD and MAY do, one requirement per bullet.
  Code, tests and reviews are held to it.
- `docs/decisions/` holds the decisions behind that text and the evidence that settled them.
  A decision is not a requirement; where the two disagree, the requirement wins.
- `docs/threat-model.md` is the privilege-boundary record: what may reach the one process that
  writes as root.
- `GLOSSARY.md` defines the vocabulary.
  A requirement uses those terms and no others.
- `ROADMAP.md` is the execution ledger: task order, states, merged pull requests and verification
  evidence.
  The `State` column below summarises it, and where the two disagree `ROADMAP.md` wins.

## Scope (v1)

- Ubuntu and derivatives, detected from `/etc/os-release`; codename and architecture rules are in
  `docs/spec/platform.md`.
- Debian: out of scope (different mirror list).
  Unsupported distro -> exit with clear error.
- Only official Ubuntu archive URIs are rewritten (and `apt.pop-os.org/ubuntu` on Pop!_OS).
  `security.ubuntu.com`, PPAs, third-party repos are never touched.

## Requirement index

One row per requirement, in file order. An id is assigned once and never renumbered.
`State` is `planned` where the code does not implement the requirement at all,
and `partial:` with what is missing where it implements only part of it.

| ID | File | Requirement | State |
| --- | --- | --- | --- |
| `APPLY-1` | `docs/spec/apply.md` | One code path MUST write, and it MUST be internal. | implemented |
| `RESTORE-1` | `docs/spec/apply.md` | `--restore` MUST copy the newest backup back, in one root run: it reads `/var/backups/apt-scout/<backup id>/MANIFEST` itself and aborts before parsing a byte unless that path is a regular file, owned by root, mode 0600, and not a symlink. | implemented |
| `APPLY-2` | `docs/spec/apply.md` | Source discovery and inventory MUST be read-only. | implemented |
| `APPLY-3` | `docs/spec/apply.md` | The run MUST rewrite per URI token, not per file or line. | implemented |
| `APPLY-4` | `docs/spec/apply.md` | For deb822 (`/etc/apt/sources.list.d/*.sources`) the run MUST read a stanza's `URIs:` field as one or more URIs, recognize continuation lines, repeated fields, comments and stanza boundaries, and rewrite only the matching tokens. | implemented |
| `APPLY-5` | `docs/spec/apply.md` | For legacy sources (`/etc/apt/sources.list`, `*.list`) the run MUST recognize `deb`/`deb-src` lines, bracketed options such as `[arch=... signed-by=...]`, comments and URI tokens, and MUST rewrite only the URI token. | implemented |
| `APPLY-6` | `docs/spec/apply.md` | `mirror://` and `mirror+file:` URIs (older installs) MUST be left unchanged and reported with an explanation. | implemented |
| `APPLY-7` | `docs/spec/apply.md` | In the TUI the first checked mirror in rank order MUST become the active URI. | planned |
| `APPLY-8` | `docs/spec/apply.md` | Before writing, the run MUST check that the target is a regular file (do not follow symlinks), MUST keep mode and owner, and MUST leave no half-written file. | implemented |
| `APPLY-9` | `docs/spec/apply.md` | File creation MUST be planned, never incidental: the planned `/etc/apt/sources.list.d/system-security.sources` is created with `create_new` and mode 0600, and `EEXIST` MUST abort the whole apply with nothing written. | implemented |
| `APPLY-10` | `docs/spec/apply.md` | The run MUST back up every file to be changed to `/var/backups/apt-scout/<UTC timestamp>/`, directory mode 0700, and MUST refuse a symlinked directory. | implemented |
| `APPLY-11` | `docs/spec/apply.md` | The write MUST be atomic: temp file in the same directory, fsync, rename. | implemented |
| `APPLY-12` | `docs/spec/apply.md` | A custom URL (`Insert`) MUST use the scheme `http` or `https` only, and the run MUST reject userinfo, whitespace, `file:`, and loopback/private/link-local addresses; it MUST probe the URL like any candidate and MUST show a warning before apt is pointed at a user-supplied host. | implemented |
| `APPLY-13` | `docs/spec/apply.md` | The run MUST say so before any probing when no URI is rewritable (for example arm64, or a system already using a third-party mirror): scan the sources first, print every file inspected and why each entry was skipped. | implemented |
| `APPLY-14` | `docs/spec/apply.md` | Candidate hygiene: every candidate from any list source MUST be `http`/`https`, have no userinfo, and not resolve to a private, loopback or link-local address, because mirror lists are partly fetched over plain HTTP. | implemented |
| `APPLY-15` | `docs/spec/apply.md` | After an apply the run MUST suggest `sudo apt update` and MUST NOT run it. | implemented |
| `CLI-1` | `docs/spec/cli.md` | The run MUST accept `-c`, `--country <CC>` as an ISO 3166 alpha-2 override, which always wins over detection. | implemented |
| `CLI-2` | `docs/spec/cli.md` | `-a`, `--any` MUST make the run ignore the detected and the given country: it considers the mirrors the list names for every country and ranks them together. | implemented |
| `CLI-3` | `docs/spec/cli.md` | A run given `--refresh` MUST fail as an unknown option (exit 2), because the flag is not implemented, as a run given the `-R` reserved for it does. | planned |
| `CLI-4` | `docs/spec/cli.md` | A run given `-n`, `--no-geo` MUST make no geolocation lookups at all. | implemented |
| `CLI-5` | `docs/spec/cli.md` | `-t`, `--top <N>` MUST show the N best mirrors, five by default. | implemented |
| `CLI-6` | `docs/spec/cli.md` | `-s`, `--https-only` MUST make the run consider https mirrors only. | implemented |
| `CLI-7` | `docs/spec/cli.md` | A run given `--tui` MUST fail as an unknown option (exit 2), because no TUI mode exists, and the table below carries one row for it saying the same thing. | planned |
| `CLI-8` | `docs/spec/cli.md` | `-l`, `--list` MUST print the table and the planned change only, and MUST never write, whatever the uid. | implemented |
| `CLI-9` | `docs/spec/cli.md` | `-y`, `--yes` MUST skip the one confirmation before a write in a root run. | implemented |
| `CLI-10` | `docs/spec/cli.md` | `-r`, `--restore` MUST restore the newest backup instead of applying a mirror. | implemented |
| `CLI-11` | `docs/spec/cli.md` | `-v`, `--verbose` MUST print the proof behind the default report (Output): each row's AS number and same/`diff`/`unknown` match, the server's AS, each row's country and the address the run resolved, both pocket dates in full, the `-security` lag, the exact signature status, a redirect host in full, and the full text of every rejection reason. | implemented |
| `CLI-12` | `docs/spec/cli.md` | A run given `--no-color` MUST print no colour in the report, whatever the terminal is. | implemented |
| `CLI-13` | `docs/spec/cli.md` | `-V`, `--version` MUST print the one line `apt-scout <version>` and exit 0. | implemented |
| `CLI-14` | `docs/spec/cli.md` | A non-root `apt-scout` run MUST NOT write apt sources: discovery, ranking, the recommendation with its reason, the one line `sudo apt-scout`, exit 3. | implemented |
| `CLI-15` | `docs/spec/cli.md` | A root `sudo apt-scout` run MUST write apt sources: the same discovery and recommendation, one confirmation (`--yes` skips it), the write. | implemented |
| `CLI-16` | `docs/spec/cli.md` | `apt-scout --list` MUST NOT write apt sources, whatever the uid: the ranked table and the change a write would make, exit 0. | implemented |
| `CLI-17` | `docs/spec/cli.md` | `apt-scout --tui` MUST be a usage error (exit 2), exactly as `--refresh` and `-R` are, so a `--tui` run has no write, no backup and no `sudo apt-scout --tui` line to report. | planned |
| `CLI-18` | `docs/spec/cli.md` | A non-root `apt-scout --restore` run MUST NOT write apt sources: the one line `sudo apt-scout --restore`, exit 3. | implemented |
| `CLI-19` | `docs/spec/cli.md` | A root `sudo apt-scout --restore` run MUST write apt sources: the newest backup, after one confirmation (`--yes` skips it). | implemented |
| `CLI-20` | `docs/spec/cli.md` | Every run of the mirror pipeline MUST print the ranked table, then the recommendation and its reason: host, the mirror's own country, how far `behind` it is, speed, `latency` and signature status. | implemented |
| `CLI-21` | `docs/spec/cli.md` | A write MUST confirm once: the files it will change, the old URI and the new one, and the backup path it will create. | implemented |
| `CLI-22` | `docs/spec/cli.md` | A non-root run without `--list` MUST print the table, the recommendation and one line, then exit 3: `sudo apt-scout` with the flags of that run repeated. | implemented |
| `CLI-23` | `docs/spec/cli.md` | `--tui` MUST be opt-in and MUST never be chosen for the user (not implemented yet: a run given it exits 2). | planned |
| `CLI-24` | `docs/spec/cli.md` | `--restore` MUST replace the source of the rewrite: the newest backup instead of the mirror pipeline. | implemented |
| `CLI-25` | `docs/spec/cli.md` | The write MUST NOT have a step the user types. | implemented |
| `ERR-1` | `docs/spec/errors.md` | Every error message MUST state what failed, which path/URL/value, why (the underlying OS or HTTP error, kept verbatim), and how to fix it. | implemented |
| `ERR-2` | `docs/spec/errors.md` | Error types MUST carry context at the failure point (path, URL, step name) and MUST never discard the source error. | implemented |
| `ERR-3` | `docs/spec/errors.md` | The program MUST self-heal automatically where that is safe, and tell the user in one line what was done. | implemented |
| `ERR-4` | `docs/spec/errors.md` | The program MUST NOT keep a cache in v1, so there is none to heal: no cache directory and no cache file exist, and the mirror list, the geo answers and the AS lookups are fetched afresh on every run, so no stale copy is ever served. | implemented |
| `ERR-5` | `docs/spec/errors.md` | The program MUST NOT self-heal when it is unsafe: it MUST never guess which apt file to overwrite, MUST never continue after a failed backup, and self-healing MUST never create a file in `/etc/apt`. | implemented |
| `ERR-6` | `docs/spec/errors.md` | The run MUST NOT fall back to another data source silently: whenever a fallback is used, it MUST print which one and why. | implemented |
| `ERR-7` | `docs/spec/errors.md` | Every exit code MUST accompany a message that explains the cause (see Non-functional). | implemented |
| `ERR-8` | `docs/spec/errors.md` | The test suite MUST cover the message content for the common failures (missing dir, permission denied, unreachable network, unknown distro). | implemented |
| `ERR-9` | `docs/spec/errors.md` | Docker integration tests for source discovery/apply/restore MUST use disposable Ubuntu containers and isolated `/etc/apt` fixtures. | implemented |
| `LOC-1` | `docs/spec/localization.md` | A locale MUST select the words a run prints and nothing else. | planned |
| `LOC-2` | `docs/spec/localization.md` | English MUST be the default and the fallback, and every user-facing message MUST have an English text. | planned |
| `LOC-3` | `docs/spec/localization.md` | Message ids MUST be stable and MUST never be keyed on English text. | planned |
| `LOC-4` | `docs/spec/localization.md` | Selection MUST be RFC 4647 lookup: `LC_ALL` when set and non-empty, else `LANG`, else English. | planned |
| `LOC-5` | `docs/spec/localization.md` | Chinese MUST be selected by script, before the lookup, and a script MUST never be substituted. | planned |
| `LOC-6` | `docs/spec/localization.md` | The catalogs MUST be Fluent files compiled into the binary. | planned |
| `LOC-7` | `docs/spec/localization.md` | The dependency set MUST change once, by decision, and `cargo deny check` MUST be what approves it. | planned |
| `LOC-8` | `docs/spec/localization.md` | A catalog MUST be complete or it MUST NOT be shipped. | planned |
| `LOC-9` | `docs/spec/localization.md` | A catalog that does not parse MUST NOT ship. | planned |
| `LOC-10` | `docs/spec/localization.md` | A counted message MUST be one Fluent message with a select expression over the counted variable, and the plural category MUST be chosen by the library from the catalog's own language; the crate MUST carry no plural rule of its own. | planned |
| `LOC-11` | `docs/spec/localization.md` | The run MUST NOT translate mirror hostnames, URIs, package names, file paths, option names, country codes, AS numbers, the program name and the `sudo apt-scout ...` line a non-root run prints: a user copies that line, so it stays runnable. | planned |
| `LOC-12` | `docs/spec/localization.md` | Column alignment MUST count display columns, not characters, so a double-width script lines up in the ranked table like a Latin one. | planned |
| `LOC-13` | `docs/spec/localization.md` | Ordering MUST never use translated text. | planned |
| `LOC-14` | `docs/spec/localization.md` | Every release MUST carry the English manual page, and a locale whose catalog ships MUST get its own page. | partial: the localized pages are missing; the English page ships |
| `NFR-1` | `docs/spec/non-functional.md` | Every release MUST ship one static musl binary: `ureq` with rustls, `fluent-templates` for the message catalogs, `ratatui` + `crossterm` for the TUI. | partial: only ureq ships; the catalog and TUI dependencies do not |
| `NFR-2` | `docs/spec/non-functional.md` | English MUST be the default and the fallback for every message, help text, error and manual. | implemented |
| `NFR-3` | `docs/spec/non-functional.md` | Every release MUST ship a comprehensive Linux man page at `man/apt-scout.1`, accessible as `man apt-scout`. | implemented |
| `NFR-4` | `docs/spec/non-functional.md` | The run MUST exit with these codes: 0 ok, 1 runtime error, 2 usage error, 3 the run would write and this uid may not, and 4 no usable distro, architecture or release — an unsupported distro or architecture, or a release that left `archive.ubuntu.com` for `old-releases.ubuntu.com` because the reference `<codename>-updates` answered 404. | implemented |
| `NFR-5` | `docs/spec/non-functional.md` | The delivery sequence MUST live in `ROADMAP.md` alone: the task order, task states, dependencies, merged pull requests and milestone audit gates are the ledger's, and this chapter states requirements only. | implemented |
| `NFR-6` | `docs/spec/non-functional.md` | The crate SHOULD follow this layout: `main.rs`, `error.rs`, `distro.rs`, `geo.rs`, `mirrors.rs`, `probe.rs`, `fresh.rs`, `verify.rs`, `rank.rs`, `sources.rs` (pure string in/out), `apply.rs`, `l10n.rs` (locale tag from two injected values, the compiled catalog loader, message lookup, display width), `locales/<tag>/main.ftl` (one Fluent catalog per locale, `en` the source of truth), `tui/`. | partial: the layout names files that do not exist yet |
| `NFR-7` | `docs/spec/non-functional.md` | The test suite MUST include these fixtures: Ubuntu 22.04 `sources.list`, 24.04 `ubuntu.sources`, multi-URI stanza, `Enabled: no`, comments, `deb-src`, arm64 ports, `InRelease`, mirror list HTML and text. | planned |
| `NFR-8` | `docs/spec/non-functional.md` | Parsing and ranking MUST be pure functions, unit-tested on fixtures, and a test MUST NOT use the live network. | implemented |
| `NFR-9` | `docs/spec/non-functional.md` | The run MUST treat every geolocation and AS answer as untrusted input: it MUST validate it, and it MAY influence ranking only. | implemented |
| `NFR-10` | `docs/spec/non-functional.md` | Freshness trust boundary: ranking MAY display and use an `InRelease` `Date` even when signature verification is `invalid` or `unavailable`; that can mis-rank a mirror but can never authorize package installation. | implemented |
| `OUT-1` | `docs/spec/output.md` | The default report MUST be the decision, and `--verbose` MUST be the proof. | implemented |
| `OUT-2` | `docs/spec/output.md` | The ranked table's columns MUST be named for what they measure. | implemented |
| `OUT-3` | `docs/spec/output.md` | The trailing `notes` cell MUST carry exactly one fact, and a column that would be empty MUST be dropped. | implemented |
| `OUT-4` | `docs/spec/output.md` | A run MUST write its report once, on stdout, once probing and ranking are done: the identity line, `Sources`, the ranked section with its title, `Detail` in a `--verbose` run, `Recommendation`, `Planned change` when the mode plans one, in that order, and `Rejected` only when something was rejected. | implemented |
| `OUT-5` | `docs/spec/output.md` | Colour MUST be display-only. | implemented |
| `OUT-6` | `docs/spec/output.md` | The accent MUST have two forms, `COLORTERM` MUST pick between them, and the fallback MUST be named because a terminal will not complain. | implemented |
| `OUT-7` | `docs/spec/output.md` | The plain output MUST be the contract, and colour MUST reach no decision. | implemented |
| `OUT-8` | `docs/spec/output.md` | Progress MUST be transient, animated, and only for a person. | implemented |
| `OUT-9` | `docs/spec/output.md` | The animation MUST be testable without a terminal, and it MUST add no dependency. | implemented |
| `OUT-10` | `docs/spec/output.md` | Progress MUST end with one summary line. | implemented |
| `OUT-11` | `docs/spec/output.md` | `NO_COLOR`, `COLORTERM` and, later, the locale MUST be the only environment reads that may change printed text. | implemented |
| `OUT-12` | `docs/spec/output.md` | Text the run did not author MUST reach a terminal with every control character replaced, so a mirror cannot write an escape sequence into it. | partial: no end-to-end container proof; D90 |
| `PIPE-1` | `docs/spec/pipeline.md` | The run MUST detect the distro from `/etc/os-release`. | implemented |
| `PIPE-2` | `docs/spec/pipeline.md` | The run MUST detect the location of the server (country, then AS), skipped with `--no-geo`. | partial: RIPEstat is not implemented |
| `PIPE-3` | `docs/spec/pipeline.md` | The run MUST load the mirror list, the first success wins, and the source it used MUST be shown in the output. | partial: the 24 h cache and the compiled snapshot are not implemented |
| `PIPE-4` | `docs/spec/pipeline.md` | The run MUST filter to the mirrors in the detected country only, unless `--any` is given, which removes the country from the candidate set; plus the one automatic widening described below, which adds every other country's mirrors once and only when the run's own country came back with nothing usable. | implemented |
| `PIPE-5` | `docs/spec/pipeline.md` | The run MUST resolve the AS of the candidates (skipped with `--no-geo`; informational, never a reason to fail the run): resolve each hostname to an IP, then Team Cymru DNS TXT per IP (parallel, cap 8 at a time), normalized as in step 2. | partial: the AS cache and IPv6-specific selection are not implemented |
| `PIPE-6` | `docs/spec/pipeline.md` | The run MUST fetch the reference once, before probing: from `http://archive.ubuntu.com/ubuntu/dists/<codename>-updates/InRelease` and `<codename>-security/InRelease` get the `Date:` of each pocket. | implemented |
| `PIPE-7` | `docs/spec/pipeline.md` | The run MUST probe each candidate in parallel (`std::thread`): fetch `<mirror>/dists/<codename>-updates/InRelease` in full under the 512 KiB download/verification cap so its OpenPGP signature can be checked, and fetch only the head (`Range: bytes=0-4095`) of `<mirror>/dists/<codename>-security/InRelease` for the informational `Date`. | implemented |
| `PIPE-8` | `docs/spec/pipeline.md` | The run MUST reject unreachable mirrors, non-200 responses, and stale mirrors. | implemented |
| `PIPE-9` | `docs/spec/pipeline.md` | The run MUST rank by comparing the `-updates` `Date` exactly (no tolerance); lag is the reference `Date` minus the mirror `Date`, clamped to 0 because the reference is fetched first and a mirror may be newer. | implemented |
| `PIPE-10` | `docs/spec/pipeline.md` | The run MUST build the plan described in Apply, validate it against the closed target enumeration and the current on-disk digests, and only then write. | implemented |
| `PLAT-1` | `docs/spec/platform.md` | The run MUST support Ubuntu and its derivatives, detected from `/etc/os-release` `ID` / `ID_LIKE`. | implemented |
| `PLAT-2` | `docs/spec/platform.md` | The run MUST support Pop!_OS (`ID=pop`, `ID_LIKE="ubuntu debian"`, `UBUNTU_CODENAME` equals the Ubuntu base, for example `jammy`). | implemented |
| `PLAT-3` | `docs/spec/platform.md` | The run MUST accept the architectures `amd64` and `i386` only (from `dpkg --print-architecture`). | implemented |
| `TEST-1` | `docs/spec/testing.md` | An assertion MUST compare a value with an expectation that does not derive from the same expression as the value. | implemented |
| `TEST-2` | `docs/spec/testing.md` | Unit tests (`#[cfg(test)]`) MUST cover pure logic on fixtures: `os-release`, Launchpad HTML, `mirrors.txt`, `InRelease` dates, freshness, ranking, read-only apt source inventory/parsers, and D8 source rewrite output/round trips. | implemented |
| `TEST-3` | `docs/spec/testing.md` | Docker integration tests MUST run inside disposable real Ubuntu images, and MUST include offline cases and live-network smoke cases. | implemented |
| `TEST-4` | `docs/spec/testing.md` | Localization MUST be tested in two layers, and a catalog MUST ship only when both pass. | planned |
| `TEST-5` | `docs/spec/testing.md` | One script (`scripts/docker-test.sh`) MUST run the container tests on the developer's machine only, and CI MUST NOT run Docker tests (CI builds and runs static checks). | implemented |
| `TEST-6` | `docs/spec/testing.md` | Pop!_OS MUST be tested in an Ubuntu 22.04 container with the real `pop-default-settings` package downloaded from `apt.pop-os.org` (pinned by file checksum in the test fixtures, downloaded once and cached; the download is a test setup step, not a test of the live service): its `os-release` and the deb822 `system.sources` template from its `postinst` are the fixture. | implemented |
| `TEST-7` | `docs/spec/testing.md` | Live-network smoke tests (real Launchpad, geo sources) MUST be manual and MUST never be part of CI, because they are rate limited and flaky. | implemented |
| `TEST-8` | `docs/spec/testing.md` | Man-page verification MUST check `man/apt-scout.1` formatting (for example with `mandoc -Tlint`) and MUST confirm `man apt-scout` renders in a disposable Ubuntu container. | planned |
| `TEST-9` | `docs/spec/testing.md` | Every call site that rewrites a control character MUST be pinned by a test, and the count of those sites MUST be asserted by a test rather than stated in prose. | partial: six call sites are escaped and unpinned; D92 |
| `TEST-10` | `docs/spec/testing.md` | A container case MUST prove that a control character in a mirror URL reaches the terminal as visible text, and MUST fail on a raw control byte. | implemented |
| `TUI-1` | `docs/spec/tui.md` | The TUI MUST show a ranked mirror table (left/top), a detail panel for the highlighted mirror, and a footer with the active keys. | planned |
| `TUI-2` | `docs/spec/tui.md` | The table MUST carry these columns: checkbox, rank, host, country, scheme, latency, throughput, lag (updates), AS match, signature, `Recommended` badge. | planned |
| `TUI-3` | `docs/spec/tui.md` | The detail panel MUST show the full URL, country, AS number and name, resolved IP, scheme, time to first byte, throughput, `Date` and lag for `-updates` (decides) and `-security` (informational), AS match, signature status of the `-updates` `InRelease`, mirror list source, status or rejection reason. | planned |
| `TUI-4` | `docs/spec/tui.md` | The best-ranked mirror MUST be pre-checked and MUST carry the `Recommended` badge. | planned |
| `TUI-5` | `docs/spec/tui.md` | The user MUST be able to check several mirrors. | planned |
| `TUI-6` | `docs/spec/tui.md` | The TUI MUST implement these keys (arrows are primary; `j/k` MAY be omitted). | planned |
| `TUI-7` | `docs/spec/tui.md` | Zero-effort flow: defaults MUST be right, so pressing `Enter` through the flow MUST save the best fresh mirror among the existing candidates. | planned |
| `TUI-8` | `docs/spec/tui.md` | Extra steps MUST appear only when the user must be informed or decide: selection has warnings (e.g. only http, lag near `MAX_LAG`, different country), a custom mirror failed its probe, no fresh mirror found, the write failed (show the reason and the state of the backup, which is untouched). | planned |
| `TUI-9` | `docs/spec/tui.md` | Every dialog MUST show its available keys in a footer bar. | planned |
| `TUI-10` | `docs/spec/tui.md` | Applying from the TUI MUST use the same writer as the default run: as root, `Enter` on the confirmation validates the plan and writes it; as a non-root uid the TUI writes nothing and prints the `sudo apt-scout --tui` line. | planned |
| `TUI-11` | `docs/spec/tui.md` | The TUI MUST restore the terminal state on exit, panic or Ctrl-C. | planned |

A row that claims `implemented` is a claim about shipped code, and `ROADMAP.md`
records the pull request, and the evidence behind each line is archived under `docs/history/`;
where the two disagree, `ROADMAP.md` wins.

## Privilege boundary and threat model

- Security goal: untrusted inputs must not let an attacker execute code as root or write outside
  the explicitly allowed apt paths through apt-scout.
- Treat CLI arguments/environment, apt source contents, network responses, mirror URLs, backup
  manifests, file metadata/races, and terminal input as untrusted.
- One process writes, and the accepted cost is stated here rather than hidden.
  In a root run the networked discovery and the parsing of untrusted responses — geolocation, mirror
  lists, `InRelease` bodies — happen in the same root process that later writes, because that is the
  workflow the user chose when running `sudo apt-scout`.
  What defends the user is the order of operations: nothing is written until the selected mirror's
  `InRelease` verifies and the internal plan validates against the on-disk state.
  A two-process split that keeps the network phase out of root is future work and is not a supported
  flag in v1.
- The write path must use fixed executables/paths and a minimal controlled environment; it must
  not invoke a shell and must not resolve a command through `PATH`, and after verification it spawns
  no external process.
  Validate file type, ownership/mode policy, path containment, and symlink/TOCTOU behavior at the
  point of use; fail closed on any mismatch.
- The program never re-execs itself privileged, never invokes `sudo`, and is never installed setuid.
  Privilege is obtained only by the user running the program as root.
  A root run is the same binary with the same code; the only thing that changes is that it may write.
  Security review reduces risk but is not a guarantee of absolute immunity.
- What a write is bound to: the bytes it writes are the bytes its own validation passed, over a
  disk state that still matches the digests it computed.
  Every target's new content carries a digest the writer recomputes, and a mismatch is refused.
  The confirmation prompt is a safety rail, not a security control: the guarantee is the validation,
  and `--yes` removes only the prompt.
- The writer is one code path in one binary, shared by the default run, the TUI confirm, and
  `--restore`.
  A separate privileged artifact is out of scope for v1, as is a two-process split; either would be
  a new product decision, not something a size measurement or a code review finding can trigger.
- Before D9 or any write implementation, D6c, D6e, and D6f must be complete.
  Every write PR needs a security-focused exact-diff review and disposable Ubuntu-container tests
  for path traversal, malformed manifests, symlinks/races, backup failures without partial writes,
  rollback, and rejection of paths outside `/etc/apt`.
  Never run write tests on the developer host.

## Resilience

Third-party services change formats, add limits or become paid.
The program must keep working when they do.

- Geolocation and AS lookups are optional.
  If country detection fails, the program asks for `--country`, or runs with `--any`; it never
  silently guesses a worldwide list, and the one widening `docs/spec/pipeline.md` step 4 describes is announced, refused
  when the loaded list names one country, and never a substitute for a detected country.
  If AS lookup fails, ranking proceeds without AS preference.
  Failures include source and reason.
- Core data comes from sources the project depends on anyway: the Ubuntu archive (`InRelease`),
  Launchpad mirror list, DNS.
  No account, key or paid tier is needed anywhere.
- Parsers reject unexpected shapes instead of guessing, and report which field was missing.
- Weekly contract check (`.github/workflows/source-contract.yml` runs
  `.github/scripts/check-sources.sh`; the script can be run locally; not a required check): fetch
  each live source once, check that the fields the parser needs are still present, and open an issue
  only after two consecutive failures (runner IPs can be rate limited by individual services).
  Scheduled workflows on public repos are disabled after 60 days without repository activity; the
  check is advisory.

## Out of scope (v1)

Debian, Fedora/other package managers, running `apt update`, scheduling, GUI, PPA mirrors.
