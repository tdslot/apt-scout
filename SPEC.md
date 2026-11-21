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
| `APPLY-1` | `docs/spec/apply.md` | One code path writes, and it is internal. | implemented |
| `RESTORE-1` | `docs/spec/apply.md` | `--restore` copies the newest backup back, in one root run: it reads `/var/backups/apt-scout/<backup id>/MANIFEST` itself and aborts before parsing a… | implemented |
| `APPLY-2` | `docs/spec/apply.md` | Source discovery and inventory are read-only. | implemented |
| `APPLY-3` | `docs/spec/apply.md` | Rewrite per URI token, not per file or line. | implemented |
| `APPLY-4` | `docs/spec/apply.md` | deb822 (`/etc/apt/sources.list.d/*.sources`): a stanza's `URIs:` field may hold several URIs; D7 recognizes continuation lines, repeated fields,… | implemented |
| `APPLY-5` | `docs/spec/apply.md` | Legacy (`/etc/apt/sources.list`, `*.list`): D7 recognizes `deb`/`deb-src` lines, bracketed options such as `[arch=... | implemented |
| `APPLY-6` | `docs/spec/apply.md` | `mirror://` / `mirror+file:` URIs (older installs): leave unchanged and report them with an explanation. | implemented |
| `APPLY-7` | `docs/spec/apply.md` | Alternatives: in the TUI, the first checked mirror in rank order becomes the active URI. | planned |
| `APPLY-8` | `docs/spec/apply.md` | Safety checks before writing: target is a regular file (do not follow symlinks), keep mode and owner, no half-written files. | implemented |
| `APPLY-9` | `docs/spec/apply.md` | File creation is planned, never incidental: `/etc/apt/sources.list.d/system-security.sources` is created with `create_new` and mode 0600, and `EEXIST` aborts the whole apply with nothing written. | implemented |
| `APPLY-10` | `docs/spec/apply.md` | Backup every file to be changed to `/var/backups/apt-scout/<UTC timestamp>/`, directory mode 0700, refuse a symlinked directory. | implemented |
| `APPLY-11` | `docs/spec/apply.md` | Write atomically: temp file in the same directory, fsync, rename. | implemented |
| `APPLY-12` | `docs/spec/apply.md` | Custom URLs (`Insert`): scheme `http` or `https` only; reject userinfo, whitespace, `file:`, and loopback/private/link-local addresses; probe like… | implemented |
| `APPLY-13` | `docs/spec/apply.md` | If no URI is rewritable (for example arm64, or a system already using a third-party mirror), say so before any probing: scan the sources first, print… | implemented |
| `APPLY-14` | `docs/spec/apply.md` | Candidate hygiene (mirror lists are partly fetched over plain HTTP): every candidate, from any list source, must be `http`/`https`, have no userinfo,… | implemented |
| `APPLY-15` | `docs/spec/apply.md` | After apply, suggest `sudo apt update`; do not run it. | implemented |
| `CLI-1` | `docs/spec/cli.md` | `-c`, `--country <CC>` — ISO 3166 alpha-2 override. | implemented |
| `CLI-2` | `docs/spec/cli.md` | `-a`, `--any` — Ignore the detected and the given country: consider the mirrors the list names for every country and rank them together. | implemented |
| `CLI-3` | `docs/spec/cli.md` | `--refresh` — Not implemented: a run given it fails as an unknown option (exit 2), as a run given the `-R` reserved for it does. | planned |
| `CLI-4` | `docs/spec/cli.md` | `-n`, `--no-geo` — No geolocation lookups at all. | implemented |
| `CLI-5` | `docs/spec/cli.md` | `-t`, `--top <N>` — Show N best mirrors (default 5). | implemented |
| `CLI-6` | `docs/spec/cli.md` | `-s`, `--https-only` — Only consider https mirrors. | implemented |
| `CLI-7` | `docs/spec/cli.md` | `--tui` — Not implemented: a run given it fails as an unknown option (exit 2), so no TUI mode exists, and the table below carries one row for it saying the same thing. | planned |
| `CLI-8` | `docs/spec/cli.md` | `-l`, `--list` — Table and planned change only; never writes, whatever the uid. | implemented |
| `CLI-9` | `docs/spec/cli.md` | `-y`, `--yes` — Skip the one confirmation before a write in a root run. | implemented |
| `CLI-10` | `docs/spec/cli.md` | `-r`, `--restore` — Restore the newest backup instead of applying a mirror. | implemented |
| `CLI-11` | `docs/spec/cli.md` | `-v`, `--verbose` — The proof behind the default report (Output): each row's AS number and same/`diff`/`unknown` match, the server's AS, each row's… | implemented |
| `CLI-12` | `docs/spec/cli.md` | `--no-color` — No colour in the report, whatever the terminal is. | implemented |
| `CLI-13` | `docs/spec/cli.md` | `-V`, `--version` — Print the one line `apt-scout <version>` and exit 0. | implemented |
| `CLI-14` | `docs/spec/cli.md` | `apt-scout` (non-root) does not write: discovery, ranking, the recommendation with its reason, the one line `sudo apt-scout`, exit 3. | implemented |
| `CLI-15` | `docs/spec/cli.md` | `sudo apt-scout` (root) writes: the same discovery and recommendation, one confirmation (`--yes` skips it), the write. | implemented |
| `CLI-16` | `docs/spec/cli.md` | `apt-scout --list` (any) does not write: the ranked table and the change a write would make, exit 0. | implemented |
| `CLI-17` | `docs/spec/cli.md` | `apt-scout --tui` (any) — Not implemented: a usage error (exit 2), exactly as `--refresh` and `-R` are, so a `--tui` run has no write, no backup and no `sudo apt-scout --tui` line to report. | planned |
| `CLI-18` | `docs/spec/cli.md` | `apt-scout --restore` (non-root) does not write: the one line `sudo apt-scout --restore`, exit 3. | implemented |
| `CLI-19` | `docs/spec/cli.md` | `sudo apt-scout --restore` (root) writes: the newest backup, after one confirmation (`--yes` skips it). | implemented |
| `CLI-20` | `docs/spec/cli.md` | Every run of the mirror pipeline prints the ranked table, then the recommendation and its reason: host, the mirror's own country, how far `behind` it is, speed, `latency` and signature status. | implemented |
| `CLI-21` | `docs/spec/cli.md` | A write confirms once: the files it will change, the old URI and the new one, and the backup path it will create. | implemented |
| `CLI-22` | `docs/spec/cli.md` | A non-root run without `--list` prints the table, the recommendation and one line, then exits 3: `sudo apt-scout` with the flags of that run repeated. | implemented |
| `CLI-23` | `docs/spec/cli.md` | `--tui` is opt-in and is never chosen for the user (not implemented yet: a run given it exits 2). | planned |
| `CLI-24` | `docs/spec/cli.md` | `--restore` replaces the source of the rewrite: the newest backup instead of the mirror pipeline. | implemented |
| `CLI-25` | `docs/spec/cli.md` | No step of the write is a mode the user types. | implemented |
| `ERR-1` | `docs/spec/errors.md` | Every error message states: what failed, which path/URL/value, why (the underlying OS or HTTP error, kept verbatim), and how to fix it. | implemented |
| `ERR-2` | `docs/spec/errors.md` | Error types carry context at the failure point (path, URL, step name); never discard the source error. | implemented |
| `ERR-3` | `docs/spec/errors.md` | Self-heal automatically (and tell the user in one line what was done): … | implemented |
| `ERR-4` | `docs/spec/errors.md` | There is no cache to heal, because v1 keeps none: no cache directory and no cache file exist, so the mirror list, the geo answers and the AS lookups… | implemented |
| `ERR-5` | `docs/spec/errors.md` | Do not self-heal when it is unsafe: never guess which apt file to overwrite, never continue after a failed backup, and self-healing never creates a file in `/etc/apt`. | implemented |
| `ERR-6` | `docs/spec/errors.md` | No silent data-source fallbacks: whenever a fallback is used, print which one and why. | implemented |
| `ERR-7` | `docs/spec/errors.md` | Exit codes (see Non-functional) always accompany a message that explains the cause. | implemented |
| `ERR-8` | `docs/spec/errors.md` | Tests cover the message content for the common failures (missing dir, permission denied, unreachable network, unknown distro). | implemented |
| `ERR-9` | `docs/spec/errors.md` | Docker integration tests for source discovery/apply/restore must use disposable Ubuntu containers and isolated `/etc/apt` fixtures. | implemented |
| `LOC-1` | `docs/spec/localization.md` | Display text only. | planned |
| `LOC-2` | `docs/spec/localization.md` | English is the default and the fallback. | planned |
| `LOC-3` | `docs/spec/localization.md` | Message ids are stable and are never keyed on English text. | planned |
| `LOC-4` | `docs/spec/localization.md` | Selection is RFC 4647 lookup. | planned |
| `LOC-5` | `docs/spec/localization.md` | Chinese is selected by script, before the lookup, and a script is never substituted. | planned |
| `LOC-6` | `docs/spec/localization.md` | The catalogs are Fluent files compiled into the binary. | planned |
| `LOC-7` | `docs/spec/localization.md` | The dependency set changes once, by decision, and `cargo deny check` is what approves it. | planned |
| `LOC-8` | `docs/spec/localization.md` | A catalog is complete or it is not shipped. | planned |
| `LOC-9` | `docs/spec/localization.md` | A catalog that does not parse cannot ship. | planned |
| `LOC-10` | `docs/spec/localization.md` | Plurals and numbers. | planned |
| `LOC-11` | `docs/spec/localization.md` | Never translated. | planned |
| `LOC-12` | `docs/spec/localization.md` | Column alignment counts display columns, not characters, so a double-width script lines up in the ranked table like a Latin one. | planned |
| `LOC-13` | `docs/spec/localization.md` | Ordering never uses translated text. | planned |
| `LOC-14` | `docs/spec/localization.md` | Manuals. | partial: the localized pages are missing; the English page ships |
| `NFR-1` | `docs/spec/non-functional.md` | Static musl binary. | partial: only ureq ships; the catalog and TUI dependencies do not |
| `NFR-2` | `docs/spec/non-functional.md` | English is the default and the fallback for every message, help text, error and manual. | implemented |
| `NFR-3` | `docs/spec/non-functional.md` | Ship a comprehensive Linux man page at `man/apt-scout.1`, accessible as `man apt-scout`. | implemented |
| `NFR-4` | `docs/spec/non-functional.md` | Exit codes: 0 ok, 1 runtime error, 2 usage error, 3 the run would write and this uid may not — every non-root run other than `--list` that reaches… | implemented |
| `NFR-5` | `docs/spec/non-functional.md` | Milestones: (1) read-only distro, geo, mirror list, probe, freshness, rank, and CLI — complete (PRs #13–#16); (2) apt source discovery/rewrite,… | implemented |
| `NFR-6` | `docs/spec/non-functional.md` | Suggested layout: `main.rs`, `error.rs`, `distro.rs`, `geo.rs`, `mirrors.rs`, `probe.rs`, `fresh.rs`, `verify.rs`, `rank.rs`, `sources.rs` (pure… | partial: the layout names files that do not exist yet |
| `NFR-7` | `docs/spec/non-functional.md` | Test fixtures: Ubuntu 22.04 `sources.list`, 24.04 `ubuntu.sources`, multi-URI stanza, `Enabled: no`, comments, `deb-src`, arm64 ports, `InRelease`, mirror list HTML and text. | planned |
| `NFR-8` | `docs/spec/non-functional.md` | Parsing and ranking are pure functions, unit-tested on fixtures; no live network in tests. | implemented |
| `NFR-9` | `docs/spec/non-functional.md` | Security note: all geolocation and AS answers are untrusted input, validated, and influence ranking only. | implemented |
| `NFR-10` | `docs/spec/non-functional.md` | Freshness trust boundary: ranking may display and use an `InRelease` `Date` even when signature verification is `invalid` or `unavailable`; this can… | implemented |
| `OUT-1` | `docs/spec/output.md` | The default report is the decision; `--verbose` is the proof. | implemented |
| `OUT-2` | `docs/spec/output.md` | The ranked table's columns are named for what they measure. | implemented |
| `OUT-3` | `docs/spec/output.md` | The trailing `notes` cell carries exactly one fact, and a column that would be empty is dropped. | implemented |
| `OUT-4` | `docs/spec/output.md` | One report, one block. | implemented |
| `OUT-5` | `docs/spec/output.md` | Colour is display-only. | implemented |
| `OUT-6` | `docs/spec/output.md` | The accent has two forms, and the fallback is named because a terminal will not complain. | implemented |
| `OUT-7` | `docs/spec/output.md` | The plain output is the contract, and colour reaches no decision. | implemented |
| `OUT-8` | `docs/spec/output.md` | Progress is transient, animated, and only for a person. | implemented |
| `OUT-9` | `docs/spec/output.md` | The animation is testable without a terminal, and it adds no dependency. | implemented |
| `OUT-10` | `docs/spec/output.md` | Progress ends with one summary line. | implemented |
| `OUT-11` | `docs/spec/output.md` | `NO_COLOR`, `COLORTERM` and, later, the locale are the only environment reads that may change printed text. | implemented |
| `PIPE-1` | `docs/spec/pipeline.md` | Detect distro from `/etc/os-release`. | implemented |
| `PIPE-2` | `docs/spec/pipeline.md` | Detect location of the server (country, then AS), skipped with `--no-geo`. | partial: RIPEstat is not implemented |
| `PIPE-3` | `docs/spec/pipeline.md` | Load mirror list, first success wins, source shown in output: 1. | partial: the 24 h cache and the compiled snapshot are not implemented |
| `PIPE-4` | `docs/spec/pipeline.md` | Filter: mirrors in the detected country only, unless `--any` is given, which removes the country from the candidate set; plus the one automatic… | implemented |
| `PIPE-5` | `docs/spec/pipeline.md` | Resolve AS of candidates (skipped with `--no-geo`; informational, never a reason to fail the run): resolve each hostname to an IP, then Team Cymru… | partial: the AS cache and IPv6-specific selection are not implemented |
| `PIPE-6` | `docs/spec/pipeline.md` | Fetch reference once, before probing: from `http://archive.ubuntu.com/ubuntu/dists/<codename>-updates/InRelease` and `<codename>-security/InRelease` get the `Date:` of each pocket. | implemented |
| `PIPE-7` | `docs/spec/pipeline.md` | Probe each candidate in parallel (`std::thread`): fetch `<mirror>/dists/<codename>-updates/InRelease` in full under the 512 KiB download/verification… | implemented |
| `PIPE-8` | `docs/spec/pipeline.md` | Reject unreachable mirrors, non-200 responses, and stale mirrors. | implemented |
| `PIPE-9` | `docs/spec/pipeline.md` | Rank: compare `-updates` `Date` exactly (no tolerance); lag is the reference `Date` minus the mirror `Date`, clamped to 0 because the reference is fetched first and a mirror may be newer. | implemented |
| `PIPE-10` | `docs/spec/pipeline.md` | Plan and write: build the plan described in Apply, validate it against the closed target enumeration and the current on-disk digests, and only then write. | implemented |
| `PLAT-1` | `docs/spec/platform.md` | Ubuntu and derivatives, detected from `/etc/os-release` `ID` / `ID_LIKE`. | implemented |
| `PLAT-2` | `docs/spec/platform.md` | Pop!_OS (`ID=pop`, `ID_LIKE="ubuntu debian"`, `UBUNTU_CODENAME` equals the Ubuntu base, for example `jammy`): supported. | implemented |
| `PLAT-3` | `docs/spec/platform.md` | Architectures `amd64` and `i386` only (from `dpkg --print-architecture`). | implemented |
| `TEST-1` | `docs/spec/testing.md` | An assertion compares a value with an expectation that does not derive from the same expression as the value. | implemented |
| `TEST-2` | `docs/spec/testing.md` | Unit tests (`#[cfg(test)]`) cover pure logic on fixtures: `os-release`, Launchpad HTML, `mirrors.txt`, `InRelease` dates, freshness, ranking,… | implemented |
| `TEST-3` | `docs/spec/testing.md` | Docker integration tests run inside disposable real Ubuntu images; offline cases and live-network smoke cases are both included. | implemented |
| `TEST-4` | `docs/spec/testing.md` | Localization is tested in two layers, and a catalog ships only when both pass. | planned |
| `TEST-5` | `docs/spec/testing.md` | One script (`scripts/docker-test.sh`) runs the container tests on the developer's machine only; CI does not run Docker tests (CI builds and runs static checks). | implemented |
| `TEST-6` | `docs/spec/testing.md` | Pop!_OS is tested in an Ubuntu 22.04 container with the real `pop-default-settings` package downloaded from `apt.pop-os.org` (pinned by file checksum… | implemented |
| `TEST-7` | `docs/spec/testing.md` | Live-network smoke tests (real Launchpad, geo sources) are manual, never part of CI, because they are rate limited and flaky. | implemented |
| `TEST-8` | `docs/spec/testing.md` | Man-page verification checks `man/apt-scout.1` formatting (for example with `mandoc -Tlint`) and confirms `man apt-scout` renders in a disposable Ubuntu container. | planned |
| `TUI-1` | `docs/spec/tui.md` | Layout: ranked mirror table (left/top), a detail panel for the highlighted mirror, and a footer with the active keys. | planned |
| `TUI-2` | `docs/spec/tui.md` | Table columns: checkbox, rank, host, country, scheme, latency, throughput, lag (updates), AS match, signature, `Recommended` badge. | planned |
| `TUI-3` | `docs/spec/tui.md` | Detail panel: full URL, country, AS number and name, resolved IP, scheme, time to first byte, throughput, `Date` and lag for `-updates` (decides) and… | planned |
| `TUI-4` | `docs/spec/tui.md` | Recommendation: the best-ranked mirror is pre-checked and carries the `Recommended` badge. | planned |
| `TUI-5` | `docs/spec/tui.md` | Selection: multiple checkboxes. | planned |
| `TUI-6` | `docs/spec/tui.md` | Keys (arrows are primary; `j/k` not required): … | planned |
| `TUI-7` | `docs/spec/tui.md` | Zero-effort flow: defaults must be right, so pressing `Enter` through the flow saves the best fresh mirror among the existing candidates. | planned |
| `TUI-8` | `docs/spec/tui.md` | Extra steps appear only when the user must be informed or decide: selection has warnings (e.g. | planned |
| `TUI-9` | `docs/spec/tui.md` | Every dialog shows its available keys in a footer bar. | planned |
| `TUI-10` | `docs/spec/tui.md` | Applying from the TUI uses the same writer as the default run: as root, `Enter` on the confirmation validates the plan and writes it; as a non-root… | planned |
| `TUI-11` | `docs/spec/tui.md` | Terminal state is always restored on exit, panic or Ctrl-C. | planned |

A row that claims `implemented` is a claim about shipped code, and `ROADMAP.md`
records the pull request and the evidence behind each line; where the two disagree, `ROADMAP.md` wins.

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
