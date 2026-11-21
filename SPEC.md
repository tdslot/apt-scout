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
  It is the only place that answers "is this built yet".

## Scope (v1)

- Ubuntu and derivatives, detected from `/etc/os-release`; codename and architecture rules are in
  `docs/spec/platform.md`.
- Debian: out of scope (different mirror list).
  Unsupported distro -> exit with clear error.
- Only official Ubuntu archive URIs are rewritten (and `apt.pop-os.org/ubuntu` on Pop!_OS).
  `security.ubuntu.com`, PPAs, third-party repos are never touched.

## Requirement index

Each area owns one file and one requirement prefix.
Numbered requirements and their one-line statements are added per area.

| Prefix | File | Area | State |
| --- | --- | --- | --- |
| `CLI-` | `docs/spec/cli.md` | Command line, flags, exit codes | implemented, except `--refresh` and `--tui` |
| `OUT-` | `docs/spec/output.md` | Default report, verbose report, progress | implemented |
| `PIPE-` | `docs/spec/pipeline.md` | Shortlist, geo, probing, freshness, ranking | implemented, except the 24 h mirror-list cache |
| `APPLY-` | `docs/spec/apply.md` | Plan, confirmation, backup, atomic write | implemented |
| `RESTORE-` | `docs/spec/apply.md` | `--restore` | implemented |
| `ERR-` | `docs/spec/errors.md` | Error classes and self-healing | implemented |
| `LOC-` | `docs/spec/localization.md` | Locale selection and catalogs | not implemented |
| `TEST-` | `docs/spec/testing.md` | Unit, CLI and container test contract | implemented |
| `NFR-` | `docs/spec/non-functional.md` | Size, speed, dependencies, man page | implemented |
| `PLAT-` | `docs/spec/platform.md` | Distributions, codenames, architectures, Pop!_OS | implemented |
| `TUI-` | `docs/spec/tui.md` | Terminal UI | not implemented |

`State` is the author's reading of the code at the commit that wrote it. `ROADMAP.md` is the source
of truth and records the pull request and evidence behind each line.

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
