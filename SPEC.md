# apt-scout SPEC

Standalone Rust CLI. Picks the fastest, fresh, nearby Ubuntu archive mirror and rewrites apt sources. Equivalent of `nala fetch` without installing nala. No Python or other runtime.

## Scope (v1)

- Ubuntu and derivatives, detected from `/etc/os-release` `ID` / `ID_LIKE`. The codename is `UBUNTU_CODENAME` when present, otherwise `VERSION_CODENAME`: Linux Mint reports `VERSION_CODENAME=wilma` but apt uses Ubuntu's `noble`.
- Pop!_OS (`ID=pop`, `ID_LIKE="ubuntu debian"`, `UBUNTU_CODENAME` equals the Ubuntu base, for example `jammy`): supported. Verified from the real `pop-default-settings` package (`/etc/pop-os/os-release`, `postinst`): the active source is the deb822 file `/etc/apt/sources.list.d/system.sources` with `URIs: http://apt.pop-os.org/ubuntu`, `Suites: <codename> <codename>-security <codename>-updates <codename>-backports`, and `X-Repolib-Default-Mirror: http://apt.pop-os.org/ubuntu`. `apt.pop-os.org/ubuntu` is a true Ubuntu mirror (`Origin: Ubuntu`, same `InRelease` Date as `archive.ubuntu.com`). Rewrite rules for Pop!_OS: rewrite only the `URIs` of `system.sources`; leave `X-Repolib-Default-Mirror` unchanged (it is the vendor default kept for restore, as `apt-manage` treats it); never touch System76's own repositories (`apt.pop-os.org/release`, `apt.pop-os.org/proprietary`, `pop-os-apps.sources`). `apt.pop-os.org/ubuntu` is treated as an official archive URI for Pop!_OS only. Because Pop!_OS keeps `-security` in the same stanza as the other suites, the rewrite moves it: `system.sources` keeps the non-security suites with the new mirror, and a new file `/etc/apt/sources.list.d/system-security.sources` holds a stanza for the `-security` suite on the original mirror (this is the one case where apt-scout creates a file in `/etc/apt/sources.list.d`; it is recorded in the backup manifest and removed by `--restore`). The new stanza gets a different `X-Repolib-Name` (`Pop_OS Security`) and copies `Types`, `Components`, `Enabled` and `Signed-By` from the original. Verified in a container with the real repolib 2.2.2 package: two stanzas with the same `X-Repolib-Name` in one file are merged on `load()`+`save()` (the first stanza gains the `-security` suite), so splitting inside one file is unsafe; separate files with different names load and save unchanged. The diff shows the split before anything is written. `repolib`/`apt-manage` may add `X-Repolib-ID: system` on save; that is harmless. Tools such as `pop-upgrade` or `apt-manage` can later reset the URIs to the vendor default; the rewrite is then simply undone, which is acceptable. Check whether the `pop-default-settings` `postinst` overwrites `system.sources` on package upgrade (it only creates the file when missing, so an upgrade does not overwrite it).
- Debian: out of scope (different mirror list). Unsupported distro -> exit with clear error.
- Only official Ubuntu archive URIs are rewritten (and `apt.pop-os.org/ubuntu` on Pop!_OS). `security.ubuntu.com`, PPAs, third-party repos are never touched.
- Architectures `amd64` and `i386` only (from `dpkg --print-architecture`). Other architectures (arm64, ...) use `ports.ubuntu.com`, which has no mirror choice: print an explanation and make no changes.

## CLI

Single command, no subcommands: `apt-scout [flags]`

| Flag | Meaning |
| --- | --- |
| `--country <CC>` | ISO 3166 alpha-2 override. Always wins over detection. |
| `--refresh` | Ignore the geo, mirror list and AS caches for this run (and rewrite them). |
| `--no-geo` | No geolocation lookups at all. Requires `--country`; AS preference is skipped. Offline-friendly for the server lookup. |
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

1. **Detect distro** from `/etc/os-release`. Read the codename: `UBUNTU_CODENAME` when present, otherwise `VERSION_CODENAME` (Linux Mint reports `VERSION_CODENAME=wilma`, but apt uses `noble`).
2. **Detect location** of the server (country, then AS), skipped with `--no-geo`. Every source is optional: if all fail, the run continues with the worldwide mirror list and live probing (see Resilience). Sources are tried in order and the first valid answer wins; web sources use HTTPS except Canonical's endpoint noted below; AS comes from DNS.
   1. Country: Canonical `http://geoip.ubuntu.com/lookup` (XML, field `CountryCode`; the same endpoint the Ubuntu installer uses; plain HTTP, so treat the answer as untrusted and validate it). Fallbacks: `https://ifconfig.co/json` (`country_iso`, operator asks for at most 1 automated request per minute), `https://ipinfo.io/json` (`country`), `https://www.cloudflare.com/cdn-cgi/trace` (`loc=<CC>`).
   2. AS of the server: Team Cymru DNS, TXT query for `<reversed-ip>.origin.asn.cymru.com` (IPv6: `<nibble-reversed-ip>.origin6.asn.cymru.com`; answer `AS | prefix | CC | registry | date`; the `CC` is the registry country, not a location, so it is never used as the server country). No web API and no HTTP: plain DNS through the system resolver. Fallback: `https://stat.ripe.net/data/prefix-overview/data.json?resource=<ip>` (`data.asns[0].asn`), then ifconfig.co/ipinfo `asn`/`org` if already fetched.
   - To query Cymru the server's public IP is needed: take it from whichever HTTP source answered (geoip, ifconfig.co, ipinfo, Cloudflare `ip=`). If none answered, AS is unknown.
   - No retries: any failure (timeout, non-200 including 429, malformed body, DNS error) falls through to the next source. Timeout 5 s per request covering the whole request, response body capped at 64 KiB, worst case about 30 s (six sources). Print which sources failed and why.
   - Validation: country is uppercased and must match `^[A-Z]{2}$`; `XX` (unknown), `T1` (Tor) and `Unknown` are treated as unknown. The same rule applies to `--country`.
   - AS normalization: every source's value becomes canonical `AS<number>`. Take the first whitespace-delimited token of `org`/`as`, prefix bare integers with `AS`, validate `^AS[0-9]{1,10}$`, compare numerically. Empty or malformed means unknown. For a prefix with several origin ASNs the first listed is used; this is approximate.
   - `--country` overrides the country only; AS is still looked up unless `--no-geo`.
   - Cache: `~/.cache/apt-scout/geo.json`, valid 24 h, stores only country and AS (never the IP). `--refresh` ignores it.
   - Privacy: the web services learn the server's public IP; the DNS resolver and Team Cymru receive it inside a DNS query; step 5 also sends up to 30 candidate mirror IPs to the resolver, Cymru and RIPEstat. Document this in the README. `--no-geo` skips step 2 and step 5 entirely (no server AS to compare against); with `--no-geo` only `--country` and live probing remain.
   - `--country` replaces only the country answer; the chain still runs to learn the public IP for the AS lookup unless `--no-geo`.
3. **Load mirror list**, first success wins, source shown in output:
   1. Launchpad `https://launchpad.net/ubuntu/+archivemirrors` (HTML, ~316 KB, ~560 mirrors in ~80 countries). Measured structure: a `<tr class="head">` row with `<th colspan="2">Country</th>` starts each country; each mirror row has a `/ubuntu/+mirror/<id>` link, one `<a href>` per protocol (`https`, `http`, `ftp`, `rsync`; only `http(s)` are used), a speed (`10 Gbps`), and a status `<span class="distromirrorstatus{UP|UNKNOWN|...}">`. Launchpad's own status (`UP` = up to date) is used as a hint, never as proof of freshness. Measured: 82 country sections, 562 mirror rows, only `UP` and `UNKNOWN` statuses seen, speeds from `128 Kbps` to `100 Gbps`, and 2 rows with only `ftp`/`rsync` links (skip them, they cannot be probed over HTTP). The parser must tolerate missing protocols and unknown statuses, and is tested against a saved fixture of this page.
   2. `http://mirrors.ubuntu.com/<CC>.txt` (plain text, one URL per line, already ordered by geographic proximity, ~6 URLs, no speed or status). Used when Launchpad fails or returns no mirror for the country.
   3. Disk cache `~/.cache/apt-scout/mirrors.json` (valid 24 h; stale cache used only if all else fails, with a warning)
   4. Snapshot compiled into the binary (dated; warn when older than 90 days)
   If every source fails but the current apt mirror works, offer to probe only the current mirror instead of aborting.
   Each network source: timeout 10 s, 2 retries, response validated (non-empty, at least one well-formed `http(s)://` URL; for Launchpad, at least one country section). Invalid response counts as failure.
4. **Filter**: mirrors in detected country. If fewer than 5, top up with nearest by region then worldwide. Cap at 30 candidates. `--https-only` drops http.
5. **Resolve AS** of candidates (skipped with `--no-geo`; informational, never a reason to fail the run): resolve each hostname to an IP, then Team Cymru DNS TXT per IP (parallel, cap 8 at a time), normalized as in step 2. Fallback per IP: RIPEstat `prefix-overview`. Cymru client: hand-rolled raw UDP query (about 80 lines; std has no TXT API and an async resolver crate contradicts the no-async rule). Read nameservers from `/etc/resolv.conf` (up to 3; the systemd-resolved stub `127.0.0.53` and Docker's `127.0.0.11` handle TXT), use an absolute name with a trailing dot (no search-domain expansion), connect the UDP socket, use a random ID, ignore replies with a mismatched ID, `QR` bit or question, cap compression-pointer depth, treat NXDOMAIN as AS unknown, and treat the `TC` (truncated) bit as failure that falls through to RIPEstat (no TCP fallback). Missing `resolv.conf` means AS unknown. No HTTP batch API and no rate-limited free tier is required. Cache results on disk with the mirror list (`--refresh` ignores the cache). CDN-backed mirrors can resolve to different AS per query; AS is approximate. Resolve both A and AAAA; probe with the address family the machine can actually use, and never fail the run because IPv6 is broken.
6. **Fetch reference** once, before probing: from `http://archive.ubuntu.com/ubuntu/dists/<codename>-updates/InRelease` and `<codename>-security/InRelease` get the `Date:` of each pocket. This is the freshness baseline. The `<codename>` release pocket is not compared: its `Date` is the release date (for example `noble` shows April 2024) and never changes, so it says nothing about freshness.
7. **Probe** each candidate in parallel (`std::thread`): fetch the same two `InRelease` files from `<mirror>/dists/...` and parse `Date:`. Measure connect latency. Measure throughput on a larger sample (`Range: bytes=0-1048575` of `main/binary-<arch>/Packages.xz`), because a few KB of `InRelease` only measures TCP slow start. Redirects: follow at most 3, re-validate the final host and scheme; report mirrors that redirect to another host.
8. **Reject** unreachable mirrors, non-200 responses, and stale mirrors. `<codename>-updates` decides: a mirror is stale if its `Date` is older than the reference by more than `MAX_LAG` (24 hours). `<codename>-security` lag is shown for information only, because `security.ubuntu.com` is left untouched. A missing `-updates` pocket or unparsable `Date` rejects. `Date` is the signing time, not the sync time. If the reference host is unreachable (network error, 5xx), use the newest `Date` among candidates as baseline and print a warning. If the reference answers 404 for the codename, the release is no longer on `archive.ubuntu.com` (it moved to `old-releases.ubuntu.com`, which has no mirrors): exit 4 with a message that names the codename and explains this; never use the newest-candidate baseline in that case. Measured on 2026-11-11 (`noble-updates`): of 10 Lithuanian and Norwegian mirrors, 6 matched the reference exactly and 4 were identical to each other but one signing generation behind (about 6 hours), so a 1-hour limit would reject 40% of good mirrors. Freshness also guards against replay of old indexes; apt still verifies signatures.
9. **Rank**: compare `-updates` `Date` exactly (no tolerance); lag is the reference `Date` minus the mirror `Date`, clamped to 0 because the reference is fetched first and a mirror may be newer. Primary key: lag ascending (mirrors with equal `Date` form one group). Within a group: throughput descending, then latency ascending; a mirror in the same AS as the server wins when its throughput is within 20% of the group's fastest mirror (band measured against that top speed, so it is not chained). A lagging but not stale mirror stays selectable and shows a warning. Accepted trade-off: a slow mirror with the exact reference `Date` ranks above a much faster one that is one signing generation (about 6 hours) behind; the user can still check the faster one in the TUI. Output shows lag and AS match.
10. **Apply** (only with `--apply`): see below.

## TUI

Library: `ratatui` + `crossterm` (no async). Opens after probing finishes (a progress view is shown while probing).

- Layout: ranked mirror table (left/top), a detail panel for the highlighted mirror, and a footer with the active keys.
- Table columns: checkbox, rank, host, country, scheme, latency, throughput, lag (updates), AS match, `Recommended` badge.
- Detail panel: full URL, country, AS number and name, resolved IP, scheme, connect latency, throughput, `Date` and lag for `-updates` (decides) and `-security` (informational), AS match, mirror list source, status or rejection reason.
- Recommendation: the best-ranked mirror is pre-checked and carries the `Recommended` badge. Rejected mirrors are shown dimmed, with the reason, and cannot be checked.
- Selection: multiple checkboxes. The first checked mirror in rank order is the primary (active URI); other checked mirrors are written as commented `alternative` lines (see Apply).
- Keys (arrows are primary; `j/k` not required):
  - `Up`/`Down`: move in the focused section. `Left`/`Right`: switch tab/page within a section (e.g. the `-updates` and `-security` detail tabs) or collapse/expand details.
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
- Rewrite per URI token, not per file or line. Rewrite a URI when its host is `archive.ubuntu.com`, `*.archive.ubuntu.com`, or any host in the current mirror list (so a previously chosen mirror is recognized without any manifest). Never rewrite `security.ubuntu.com`, `ports.ubuntu.com`, PPAs or third-party hosts. A line or stanza whose suites include a `-security` suite is never rewritten to a probed mirror, even when its host is `archive.ubuntu.com`, because `-security` freshness is not checked; report each skipped entry and why. The only exception is the Pop!_OS split above, which keeps `-security` on the original mirror. On Ubuntu 24.04 and later security already lives in its own stanza (`security.ubuntu.com`), so no split is needed.
- deb822 (`/etc/apt/sources.list.d/*.sources`): a stanza's `URIs:` field may hold several URIs; rewrite only matching tokens. Keep every other field (`Types`, `Suites`, `Components`, `Signed-By`, ...) and comments byte for byte. Do not touch stanzas with `Enabled: no`.
- Legacy (`/etc/apt/sources.list`, `*.list`): keep `[arch=... signed-by=...]` options and comments; rewrite only the URI token. `deb-src` lines are rewritten like `deb` lines.
- `mirror://` / `mirror+file:` URIs (older installs): leave unchanged and report them with an explanation.
- Alternatives: in the TUI, the first checked mirror in rank order becomes the active URI. Other checked mirrors are written as commented lines `# alternative: <uri>` above the changed line, so the user can switch quickly. apt has no failover across URIs, so alternatives are documentation only.
- Safety checks before writing: target is a regular file (do not follow symlinks), keep mode and owner, no half-written files.
- Backup every file to be changed to `/var/backups/apt-scout/<UTC timestamp>/`, directory mode 0700, refuse a symlinked directory. Write a manifest of original paths. Abort the whole apply if any backup fails.
- Write atomically: temp file in the same directory, fsync, rename.
- `--restore` copies the newest backup back. It validates the manifest first: every path must be under `/etc/apt/` and be a regular file after resolution; otherwise refuse with an explanation. `--restore` does not depend on the rewrite logic.
- Custom URLs (`Insert`): scheme `http` or `https` only; reject userinfo, whitespace, `file:`, and loopback/private/link-local addresses; probe like any candidate; show a warning before apt is pointed at a user-supplied host.
- If no URI is rewritable (for example arm64, or a system already using a third-party mirror), say so before any probing: scan the sources first, print every file inspected and why each entry was skipped, exit 0 for a dry run and 1 for `--apply`. Do not probe 30 mirrors for nothing.
- Candidate hygiene (mirror lists are partly fetched over plain HTTP): every candidate, from any list source, must be `http`/`https`, have no userinfo, and not resolve to a private, loopback or link-local address. Results from the `mirrors.ubuntu.com` fallback are marked unverified in the output.
- After apply, suggest `apt update`; do not run it.

## Resilience

Third-party services change formats, add limits or become paid. The program must keep working when they do.

- Geolocation and AS are optional accelerators. If every source fails or returns something invalid, the run continues with the worldwide mirror list and ranks by measured freshness and speed. The failure is shown with the source name and reason; it is never silent and never fatal.
- Core data comes from sources the project depends on anyway: the Ubuntu archive (`InRelease`), Launchpad mirror list, DNS. No account, key or paid tier is needed anywhere.
- Parsers reject unexpected shapes instead of guessing, and report which field was missing.
- Weekly contract check (`.github/workflows/source-contract.yml` runs `.github/scripts/check-sources.sh`; the script can be run locally; not a required check): fetch each live source once, check that the fields the parser needs are still present, and open an issue only after two consecutive failures (runner IPs can be rate limited by individual services). Scheduled workflows on public repos are disabled after 60 days without repository activity; the check is advisory.

## Errors and self-healing

Principle: the program solves its own problems when safe, and otherwise says exactly what happened, where, why, and what to do. Vague messages ("Unknown error", "Something went wrong", bare `No such file`) are forbidden.

- Every error message states: **what** failed, **which path/URL/value**, **why** (the underlying OS or HTTP error, kept verbatim), and **how to fix** it. Example: `Cannot write /var/backups/apt-scout/: permission denied (os error 13). Run with sudo, or use --list for a read-only run.`
- Error types carry context at the failure point (path, URL, step name); never discard the source error. No `unwrap()` on IO or parsing.
- Self-heal automatically (and tell the user in one line what was done):
  - Missing own directories (`~/.cache/apt-scout`, `/var/backups/apt-scout`): create with `create_dir_all`.
  - Missing, empty or corrupt cache file: ignore it, refetch, rewrite it.
  - Missing `/etc/os-release`: try `/usr/lib/os-release`, then `lsb_release` fallbacks; if still unknown, ask for `--codename`/`--distro` flags.
  - Missing `UBUNTU_CODENAME` and `VERSION_CODENAME`: use the codename in the existing apt sources, then ask for `--codename`.
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
- Security note: all geolocation and AS answers are untrusted input, validated, and influence ranking only. Canonical's lookup is plain HTTP; AS data comes from DNS. Nothing derived from them is ever written into apt sources.

## Testing

- Unit tests (`#[cfg(test)]`) cover pure logic on fixtures: `os-release`, Launchpad HTML, `mirrors.txt`, `InRelease` dates, freshness, ranking, sources rewrite. Saved fixtures come from real responses (the Launchpad page is ~316 KB; keep a trimmed copy with at least two countries and every status value seen).
- Integration tests run inside Docker on real Ubuntu images, offline. The matrix covers every supported release; format measured on 2026-11-11:

  | Release | Codename | Sources format |
  | --- | --- | --- |
  | 20.04 | focal | legacy `/etc/apt/sources.list` (10 `deb` lines) |
  | 22.04 | jammy | legacy `/etc/apt/sources.list` |
  | 24.04 | noble | deb822 `/etc/apt/sources.list.d/ubuntu.sources` |
  | 26.04 | resolute | deb822 `/etc/apt/sources.list.d/ubuntu.sources` |

  Releases whose reference `InRelease` returns 404 are handled by the exit-4 rule in step 8. `focal-updates` was still receiving updates on 2026-11-11 (`Date` 2026-11-11), so freshness stays meaningful for 20.04; keep a focal fixture. `i386` is in scope but covered by fixtures only (no i386 image). The `amd64` images are used (`--platform linux/amd64`); on Apple Silicon they run under emulation, slower but equivalent. Tests exercise sources rewrite, backup, atomic write, `--restore` and error messages against real `/etc/apt` files. Network answers are injected from fixtures; automated tests never call live services.
- One script (`scripts/docker-test.sh`) runs the container tests on the developer's machine only; CI does not run Docker tests (CI builds and runs static checks). Run it before opening a PR that changes `.rs` files or build configuration, and state the result in the PR.
- Pop!_OS is tested in an Ubuntu 22.04 container with the real `pop-default-settings` package downloaded from `apt.pop-os.org` (pinned by file checksum in the test fixtures, downloaded once and cached; the download is a test setup step, not a test of the live service): its `os-release` and the deb822 `system.sources` template from its `postinst` are the fixture. Linux Mint is covered by its documented `os-release` (`ID=linuxmint`, `VERSION_CODENAME=wilma`, `UBUNTU_CODENAME=noble`) in a container. Neither is a full installation, only the files this tool reads and writes.
- Live-network smoke tests (real Launchpad, geo sources) are manual, never part of CI, because they are rate limited and flaky.

## Implementation status

Implemented and tested: os-release detection (Ubuntu family, codename), Launchpad and `mirrors.ubuntu.com` list parsing, candidate URL hygiene, `-updates`/`-security` InRelease dates, lag, ranking, per-mirror probing, and a read-only `--country <CC> [--top N] [--https-only] [--list]` run. Not yet implemented: geolocation and AS (`--country` is required), disk cache, `mirrors.ubuntu.com` fallback wiring, apply/backup/restore, TUI.

## Out of scope (v1)

Debian, Fedora/other package managers, running `apt update`, scheduling, GUI, PPA mirrors.
