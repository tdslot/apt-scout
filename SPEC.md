# apt-scout SPEC

Standalone Rust CLI. Picks the fastest, fresh, nearby Ubuntu archive mirror and rewrites apt sources. Equivalent of `nala fetch` without installing nala. No Python or other runtime.

## Scope (v1)

- Ubuntu and derivatives (Pop!_OS, Mint), detected from `/etc/os-release` `ID` / `ID_LIKE`.
- Debian: out of scope (different mirror list). Unsupported distro -> exit with clear error.
- Only official Ubuntu archive URIs are rewritten. `security.ubuntu.com`, PPAs, third-party repos are never touched.
- Architectures `amd64` and `i386` only (from `dpkg --print-architecture`). Other architectures (arm64, ...) use `ports.ubuntu.com`, which has no mirror choice: print an explanation and make no changes.

## CLI

Single command, no subcommands: `apt-scout [flags]`

| Flag | Meaning |
| --- | --- |
| `--country <CC>` | ISO 3166 alpha-2 override. Always wins over detection. |
| `--top <N>` | Show N best mirrors (default 5). |
| `--https-only` | Only consider https mirrors. |
| `--apply` | Write the best mirror into apt sources. Root needed for the write step only. |
| `--restore` | Restore the newest backup. Root needed for the write. |
| `--list` | Print ranked table only (no TUI, no write). |
| `--no-tui` | Never open the TUI; non-interactive behavior. |
| `--yes` | Skip confirmation in non-interactive `--apply` (scripts). |

Behavior by mode:
- stdout is a TTY and no `--no-tui`/`--list`/`--apply`: open the interactive TUI (see TUI).
- No TTY, or `--no-tui`, or `--list`: dry-run. Print the ranked table and the diff `--apply` would make. Nothing is written.
- `--apply` (non-interactive): write the top-ranked mirror without the TUI. Needs root; confirmation prompt unless `--yes`.

## Pipeline

1. **Detect distro** from `/etc/os-release`. Read codename (`VERSION_CODENAME`, or `UBUNTU_CODENAME` for derivatives).
2. **Detect location** of the server (country and AS), first source that answers wins, all HTTPS:
   1. `https://ifconfig.co/json` (`country_iso`, `asn`, `asn_org`). The operator asks for at most 1 automated request per minute, which one run satisfies; results are cached (see below) so repeated runs do not hit it.
   2. `https://ipinfo.io/json` (`country`, `org` = `AS<number> <name>`).
   3. `https://www.cloudflare.com/cdn-cgi/trace` (`loc=<CC>`; country only, no AS).
   `--country` skips country detection but AS is still looked up when a source gives it. Cache the result in `~/.cache/apt-scout/geo.json` for 24 h (a VPN or move changes the answer, so `--refresh` ignores the cache). If every source fails: country unknown -> use the worldwide list, AS unknown -> no AS preference; print which sources failed and why. Validate answers (country matches `^[A-Z]{2}$`, AS matches `^AS[0-9]+`) and never trust a malformed value. Each source: timeout 5 s, 1 retry. Do not use `ip-api.com` for the server lookup (HTTP only; free tier is non-commercial).
3. **Load mirror list**, first success wins, source shown in output:
   1. `http://mirrors.ubuntu.com/<CC>.txt` (plain text, one URL per line)
   2. Launchpad `https://launchpad.net/ubuntu/+archivemirrors` (HTML, parsed against a saved fixture; format must be verified before the parser is written)
   3. Disk cache `~/.cache/apt-scout/mirrors.json` (valid 24 h; stale cache used only if all else fails, with a warning)
   4. Snapshot compiled into the binary (dated; warn when older than 90 days)
   If every source fails but the current apt mirror works, offer to probe only the current mirror instead of aborting.
   Each network source: timeout 10 s, 2 retries, response validated (non-empty, well-formed `http(s)://` URLs). Invalid response counts as failure.
4. **Filter**: mirrors in detected country. If fewer than 5, top up with nearest by region then worldwide. Cap at 30 candidates. `--https-only` drops http.
5. **Resolve AS** of candidates (informational, never a reason to fail the run): resolve hostname -> IP. Primary: one `POST http://ip-api.com/batch?fields=query,as,countryCode` for up to 100 IPs (free tier: non-commercial, HTTP only, rate limited; honor `X-Rl`/`X-Ttl` response headers and stop when `X-Rl` is 0). Fallback when the batch call fails or is rate limited: `https://stat.ripe.net/data/prefix-overview/data.json?resource=<ip>` per IP, in parallel with a small cap (4 at a time), reading `data.asns[0].asn`. Cache results on disk with the mirror list. CDN-backed mirrors can resolve to different AS per query; AS is approximate. Resolve both A and AAAA; probe with the address family the machine can actually use, and never fail the run because IPv6 is broken.
6. **Fetch reference** once, before probing: from `http://archive.ubuntu.com/ubuntu/dists/<codename>{,-updates,-security}/InRelease` get the `Date:` of each pocket. This is the freshness baseline.
7. **Probe** each candidate in parallel (`std::thread`): fetch the same three `InRelease` files from `<mirror>/dists/...` and parse `Date:`. Measure connect latency. Measure throughput on a larger sample (`Range: bytes=0-1048575` of `main/binary-<arch>/Packages.xz`), because a few KB of `InRelease` only measures TCP slow start. Redirects: follow at most 3, re-validate the final host and scheme; report mirrors that redirect to another host.
8. **Reject** unreachable mirrors, non-200 responses, and out-of-date mirrors. Freshness pockets: `<codename>` and `<codename>-updates` decide rejection; `<codename>-security` lag is shown for information only, because `security.ubuntu.com` is left untouched. A mirror is out of date if a deciding pocket's `Date` is older than the reference by more than `MAX_LAG`. `MAX_LAG` is a named constant; its initial value (1 hour) is a guess and must be tuned from measured lags. `Date` is the signing time, not the sync time, so a small lag is normal. Missing deciding pocket or unparsable `Date` rejects. If the reference is unreachable, use the newest `Date` among candidates as baseline and print a warning. Output shows lag per pocket. Freshness also guards against replay of old indexes; apt still verifies signatures.
9. **Rank**: sort by throughput desc, latency asc. Within a speed band (candidates whose throughput is within 20% of the one above), a mirror in the same AS as the server wins. Output shows the AS match.
10. **Apply** (only with `--apply`): see below.

## TUI

Library: `ratatui` + `crossterm` (no async). Opens after probing finishes (a progress view is shown while probing).

- Layout: ranked mirror table (left/top), a detail panel for the highlighted mirror, and a footer with the active keys.
- Table columns: checkbox, rank, host, country, scheme, latency, throughput, max lag, AS match, `Recommended` badge.
- Detail panel: full URL, country, AS number and name, resolved IP, scheme, connect latency, throughput, `Date` and lag per pocket (release, updates, security), AS match, mirror list source, status or rejection reason.
- Recommendation: the best-ranked mirror is pre-checked and carries the `Recommended` badge. Rejected mirrors are shown dimmed, with the reason, and cannot be checked.
- Selection: multiple checkboxes. The first checked mirror in rank order is the primary (active URI); other checked mirrors are written as commented `alternative` lines (see Apply).
- Keys (arrows are primary; `j/k` not required):
  - `Up`/`Down`: move in the focused section. `Left`/`Right`: switch tab/page within a section (e.g. detail pocket tabs) or collapse/expand details.
  - `Tab`/`Shift+Tab`: cycle focus between sections (mirror table, detail panel, buttons).
  - `Space`: toggle checkbox on the highlighted mirror.
  - `Enter`: confirm the current step or dialog (accept, yes).
  - `Esc`: cancel the current action or close a dialog; on the main screen, quit without changes (asks to confirm if the selection was changed).
  - `Insert` (implemented in the last milestone): add an entry: opens an input dialog for a custom mirror URL (validated, then probed like any candidate and added to the table).
  - `Delete`/`Del` (last milestone): remove the highlighted entry from the list (asks to confirm; only affects the current run, never apt files).
  - `r`: re-probe. `?`: help.
- Zero-effort flow: defaults must be right, so pressing `Enter` through the flow saves the best fresh mirror among the existing candidates. Keep the step count minimal, but never hide information the user needs. Base path (3 steps):
  1. Mirror table with the `Recommended` mirror pre-checked and a one-line summary of what will change; `Enter` continues.
  2. Confirmation dialog `Apply changes? [Yes]`: lists the files to change, old URI -> new URI, alternatives, and the backup path; `Yes` focused; `Enter` writes (backup first). `Esc` or `Tab` then `Enter` on `No` cancels.
  3. Result screen: what was saved, which files changed, backup path, how to undo (`--restore`), suggested `apt update`; `Enter` exits.
- Extra steps appear only when the user must be informed or decide: not root (explain, offer to exit; no write), selection has warnings (e.g. only http, lag near `MAX_LAG`, different country), a custom mirror failed its probe, no fresh mirror found, write error (show what was or was not changed and the backup state).
- Every dialog shows its available keys in a footer bar.
- Applying from the TUI follows the Apply rules below, including root and backup. If not root, show a clear message and exit without changes.
- Terminal state is always restored on exit, panic or Ctrl-C.

## Apply

- Probing and the TUI run as the invoking user. Root is needed only for the write step: if not root, the program asks to re-run itself with `sudo` for the write (passing the chosen selection), or prints the exact command. Never write without root; never continue network work as root longer than needed.
- Rewrite per URI token, not per file or line. Rewrite a URI when its host is `archive.ubuntu.com`, `*.archive.ubuntu.com`, or any host in the current mirror list (so a previously chosen mirror is recognized without any manifest). Never rewrite `security.ubuntu.com`, `ports.ubuntu.com`, PPAs or third-party hosts.
- deb822 (`/etc/apt/sources.list.d/*.sources`): a stanza's `URIs:` field may hold several URIs; rewrite only matching tokens. Keep every other field (`Types`, `Suites`, `Components`, `Signed-By`, ...) and comments byte for byte. Do not touch stanzas with `Enabled: no`.
- Legacy (`/etc/apt/sources.list`, `*.list`): keep `[arch=... signed-by=...]` options and comments; rewrite only the URI token. `deb-src` lines are rewritten like `deb` lines.
- `mirror://` / `mirror+file:` URIs (older installs): leave unchanged and report them with an explanation.
- Alternatives: in the TUI, the first checked mirror in rank order becomes the active URI. Other checked mirrors are written as commented lines `# alternative: <uri>` above the changed line, so the user can switch quickly. apt has no failover across URIs, so alternatives are documentation only.
- Safety checks before writing: target is a regular file (do not follow symlinks), keep mode and owner, no half-written files.
- Backup every file to be changed to `/var/backups/apt-scout/<UTC timestamp>/`, directory mode 0700, refuse a symlinked directory. Write a manifest of original paths. Abort the whole apply if any backup fails.
- Write atomically: temp file in the same directory, fsync, rename.
- `--restore` copies the newest backup back. It validates the manifest first: every path must be under `/etc/apt/` and be a regular file after resolution; otherwise refuse with an explanation. `--restore` does not depend on the rewrite logic.
- Custom URLs (`Insert`): scheme `http` or `https` only; reject userinfo, whitespace, `file:`, and loopback/private/link-local addresses; probe like any candidate; show a warning before apt is pointed at a user-supplied host.
- After apply, suggest `apt update`; do not run it.

## Errors and self-healing

Principle: the program solves its own problems when safe, and otherwise says exactly what happened, where, why, and what to do. Vague messages ("Unknown error", "Something went wrong", bare `No such file`) are forbidden.

- Every error message states: **what** failed, **which path/URL/value**, **why** (the underlying OS or HTTP error, kept verbatim), and **how to fix** it. Example: `Cannot write /var/backups/apt-scout/: permission denied (os error 13). Run with sudo, or use --list for a read-only run.`
- Error types carry context at the failure point (path, URL, step name); never discard the source error. No `unwrap()` on IO or parsing.
- Self-heal automatically (and tell the user in one line what was done):
  - Missing own directories (`~/.cache/apt-scout`, `/var/backups/apt-scout`): create with `create_dir_all`.
  - Missing, empty or corrupt cache file: ignore it, refetch, rewrite it.
  - Missing `/etc/os-release`: try `/usr/lib/os-release`, then `lsb_release` fallbacks; if still unknown, ask for `--codename`/`--distro` flags.
  - Missing `VERSION_CODENAME`: fall back to `UBUNTU_CODENAME`, then to the codename in the existing apt sources.
  - Apt sources at an unexpected location or format: search `/etc/apt/sources.list`, `/etc/apt/sources.list.d/*.{list,sources}`, and report every file inspected and why each was skipped or used.
  - Network failure: follow the mirror-list source chain, retry, then use cache/snapshot; report each failed source with its reason.
  - Interrupted write: temp files are cleaned up on the next run; the original file is never left half-written.
- Do not self-heal when it is unsafe: never guess which apt file to overwrite, never create files in `/etc/apt` that did not exist, never continue after a failed backup. Stop and explain instead.
- No silent fallbacks: whenever a fallback is used, print which one and why.
- Exit codes (see Non-functional) always accompany a message that explains the cause.
- Tests cover the message content for the common failures (missing dir, permission denied, corrupt cache, unreachable network, unknown distro).

## Non-functional

- Static musl binary. `ureq` with rustls, `ratatui` + `crossterm` for the TUI. No async, no OpenSSL.
- All output, errors and docs in English.
- Exit codes: 0 ok, 1 runtime error, 2 usage error, 3 not root, 4 unsupported distro or architecture.
- Milestones: (1) distro, geo, mirror list, probe, freshness, rank, `--list`; (2) apply, backup, restore, sources rewrite; (3) TUI core flow; (4) TUI `Insert`/`Delete`.
- Suggested layout: `main.rs`, `error.rs`, `distro.rs`, `geo.rs`, `mirrors.rs`, `probe.rs`, `fresh.rs`, `rank.rs`, `sources.rs` (pure string in/out), `apply.rs`, `tui/`. Keep `sources`, `rank`, `fresh` pure; HTTP is injected as a function so tests use canned responses.
- Test fixtures: Ubuntu 22.04 `sources.list`, 24.04 `ubuntu.sources`, multi-URI stanza, `Enabled: no`, comments, `deb-src`, arm64 ports, `InRelease`, mirror list HTML and text, corrupt cache. Round-trip test (rewrite then restore equals original bytes), tempdir test for atomic write and failed-backup abort, `ratatui` `TestBackend` for TUI snapshots.
- Parsing and ranking are pure functions, unit-tested on fixtures; no live network in tests.
- Security note: server geolocation uses HTTPS sources; mirror AS lookups may use HTTP (`ip-api.com` batch), so AS data influences ranking only and is never trusted for anything else. All geolocation results are validated and untrusted.

## Out of scope (v1)

Debian, Fedora/other package managers, running `apt update`, scheduling, GUI, PPA mirrors.
