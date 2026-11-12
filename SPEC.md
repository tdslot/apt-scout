# apt-scout SPEC

Standalone Rust CLI. Picks the fastest, fresh, nearby Ubuntu archive mirror and rewrites apt sources. Equivalent of `nala fetch` without installing nala. No Python or other runtime.

## Scope (v1)

- Ubuntu and derivatives, detected from `/etc/os-release` `ID` / `ID_LIKE`. The codename is `UBUNTU_CODENAME` when present, otherwise `VERSION_CODENAME`: Linux Mint reports `VERSION_CODENAME=wilma` but apt uses Ubuntu's `noble`.
- Pop!_OS (`ID=pop`, `ID_LIKE="ubuntu debian"`, `UBUNTU_CODENAME` equals the Ubuntu base, for example `jammy`): supported. Verified from the real `pop-default-settings` package (`/etc/pop-os/os-release`, `postinst`): the active source is the deb822 file `/etc/apt/sources.list.d/system.sources` with `URIs: http://apt.pop-os.org/ubuntu`, `Suites: <codename> <codename>-security <codename>-updates <codename>-backports`, and `X-Repolib-Default-Mirror: http://apt.pop-os.org/ubuntu`. `apt.pop-os.org/ubuntu` is a true Ubuntu mirror (`Origin: Ubuntu`, same `InRelease` Date as `archive.ubuntu.com`). Rewrite rules for Pop!_OS: rewrite only the `URIs` of `system.sources`; leave `X-Repolib-Default-Mirror` unchanged (it is the vendor default kept for restore, as `apt-manage` treats it); never touch System76's own repositories (`apt.pop-os.org/release`, `apt.pop-os.org/proprietary`, `pop-os-apps.sources`). `apt.pop-os.org/ubuntu` is treated as an official archive URI for Pop!_OS only. Because Pop!_OS keeps `-security` in the same stanza as the other suites, the rewrite moves it: `system.sources` keeps the non-security suites with the new mirror, and a new file `/etc/apt/sources.list.d/system-security.sources` holds a stanza for the `-security` suite on the original mirror (this is the one case where apt-scout creates a file in `/etc/apt/sources.list.d`; it is recorded in the backup manifest and removed by `--restore`). The new stanza gets a different `X-Repolib-Name` (`Pop_OS Security`) and copies `Types`, `Components`, `Enabled` and `Signed-By` from the original. Verified in a container with the real repolib 2.2.2 package: two stanzas with the same `X-Repolib-Name` in one file are merged on `load()`+`save()` (the first stanza gains the `-security` suite), so splitting inside one file is unsafe; separate files with different names load and save unchanged. The pure D8 transformation returns both the rewritten `system.sources` content and the planned new `system-security.sources` content without filesystem IO; D9 owns file creation, backup and atomic write. The diff shows the split before anything is written. `repolib`/`apt-manage` may add `X-Repolib-ID: system` on save; that is harmless. Tools such as `pop-upgrade` or `apt-manage` can later reset the URIs to the vendor default; the rewrite is then simply undone, which is acceptable. Check whether the `pop-default-settings` `postinst` overwrites `system.sources` on package upgrade (it only creates the file when missing, so an upgrade does not overwrite it).
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
2. **Detect location** of the server (country, then AS), skipped with `--no-geo`. Every source is optional: if all fail, the program asks for `--country`; it does not guess a worldwide list. The mirror-list sources themselves are Launchpad for the selected country, then the `mirrors.ubuntu.com` country list. Sources are tried in order and the first valid answer wins; web sources use HTTPS except Canonical's endpoint noted below; AS comes from DNS.
   1. Country: Canonical `http://geoip.ubuntu.com/lookup` (XML, field `CountryCode`; the same endpoint the Ubuntu installer uses; plain HTTP, so treat the answer as untrusted and validate it). Fallbacks: `https://ifconfig.co/json` (`country_iso`, operator asks for at most 1 automated request per minute), `https://ipinfo.io/json` (`country`), `https://www.cloudflare.com/cdn-cgi/trace` (`loc=<CC>`).
   2. AS of the server: Team Cymru DNS, TXT query for `<reversed-ip>.origin.asn.cymru.com` (IPv6: `<nibble-reversed-ip>.origin6.asn.cymru.com`; answer `AS | prefix | CC | registry | date`; the `CC` is the registry country, not a location, so it is never used as the server country). No web API and no HTTP: plain DNS through the system resolver. Fallback in the current implementation: the already-fetched `ifconfig.co` or `ipinfo.io` `asn`/`org` fields. RIPEstat is planned, not yet implemented.
   - To query Cymru the server's public IP is needed: take it from whichever HTTP source answered (geoip, ifconfig.co, ipinfo, Cloudflare `ip=`). If none answered, AS is unknown.
   - No retries: any failure (timeout, non-200 including 429, malformed body, DNS error) falls through to the next source. HTTP requests use a 10 s whole-request timeout, including redirects. Geolocation responses are capped at 256 KiB; the mirror-list page has a 2 MiB cap. The maximum chain duration depends on which sources fail and whether later AS queries are needed; AS DNS lookup has a 12 s total per-query budget and mirror-AS ranking has a 20 s overall budget. Print which sources failed and why.
   - Validation: country is uppercased and must match `^[A-Z]{2}$`; `XX` (unknown), `T1` (Tor) and `Unknown` are treated as unknown. The same rule applies to `--country`.
   - AS normalization: every source's value becomes canonical `AS<number>`. Take the first whitespace-delimited token of `org`/`as`, prefix bare integers with `AS`, validate `^AS[0-9]{1,10}$`, compare numerically. Empty or malformed means unknown. For a prefix with several origin ASNs the first listed is used; this is approximate.
   - `--country` overrides the country only; AS is still looked up unless `--no-geo`.
   - Cache is planned, not yet implemented.
   - Privacy: live geolocation web services learn the server's public IP; system DNS and Team Cymru receive the IP inside a DNS query. Candidate mirror host IPs are sent to DNS and Team Cymru for AS lookup. `--no-geo` skips all these lookups and requires `--country`.
   - `--country` replaces only the country answer; the chain still runs to learn the public IP for the AS lookup unless `--no-geo`.
**Preflight before any network access:** validate CLI country values against the supported country table and validate the local architecture. Invalid country/unsupported architecture must fail before geolocation or any HTTP/DNS request. Resolve this before invoking geo or mirror-list sources.

3. **Load mirror list**, first success wins, source shown in output:
   1. Launchpad `https://launchpad.net/ubuntu/+archivemirrors` (HTML, ~316 KB, ~560 mirrors in ~80 countries). Measured structure: a `<tr class="head">` row with `<th colspan="2">Country</th>` starts each country; each mirror row has a `/ubuntu/+mirror/<id>` link, one `<a href>` per protocol (`https`, `http`, `ftp`, `rsync`; only `http(s)` are used), a speed (`10 Gbps`), and a status `<span class="distromirrorstatus{UP|UNKNOWN|...}">`. Launchpad's own status (`UP` = up to date) is used as a hint, never as proof of freshness. Measured: 82 country sections, 562 mirror rows, only `UP` and `UNKNOWN` statuses seen, speeds from `128 Kbps` to `100 Gbps`, and 2 rows with only `ftp`/`rsync` links (skip them, they cannot be probed over HTTP). The parser must tolerate missing protocols and unknown statuses, and is tested against a saved fixture of this page.
   2. `http://mirrors.ubuntu.com/<CC>.txt` (plain text, one URL per line, already ordered by geographic proximity, ~6 URLs, no speed or status). Used when Launchpad fails or returns no mirror for the country.
   3. Disk cache `~/.cache/apt-scout/mirrors.json` (valid 24 h; stale cache used only if all else fails, with a warning)
   4. Snapshot compiled into the binary (dated; warn when older than 90 days)

   Each network source: one 10 s time budget per call covering redirects (no retries in v1: a failure falls through to the next source), response validated (non-empty, at least one well-formed `http(s)://` URL; for Launchpad, at least one country section). Invalid response counts as failure.
4. **Filter**: mirrors in the detected country only. v1 does not top up from nearby regions or worldwide; if a country has no usable mirror, the run says so and the user picks another with `--country`. Candidates are deduplicated (ignoring a trailing slash and case), capped at 30, and `--https-only` drops http. An unknown country code (not one of Launchpad's 82 countries) is a usage error (exit 2) before any network request; it does not fall back to `mirrors.ubuntu.com/<CC>.txt`.
5. **Resolve AS** of candidates (skipped with `--no-geo`; informational, never a reason to fail the run): resolve each hostname to an IP, then Team Cymru DNS TXT per IP (parallel, cap 8 at a time), normalized as in step 2. No per-IP fallback is implemented yet; failures leave AS unknown. Cymru client: hand-rolled raw UDP query (about 80 lines; std has no TXT API and an async resolver crate contradicts the no-async rule). Read nameservers from `/etc/resolv.conf` (up to 3; the systemd-resolved stub `127.0.0.53` and Docker's `127.0.0.11` handle TXT), use an absolute name with a trailing dot (no search-domain expansion), connect the UDP socket, use a transaction ID, validate the reply ID and `QR` bit, cap compression-pointer traversal, treat NXDOMAIN as AS unknown, and treat the `TC` (truncated) bit as failure (no TCP fallback). Missing `resolv.conf` means AS unknown. No HTTP batch API and no rate-limited free tier is required. AS caching is planned, not implemented. CDN-backed mirrors can resolve to different AS per query; AS is approximate. Current implementation resolves the first address returned by the system resolver; IPv6-specific candidate selection is not implemented yet.
6. **Fetch reference** once, before probing: from `http://archive.ubuntu.com/ubuntu/dists/<codename>-updates/InRelease` and `<codename>-security/InRelease` get the `Date:` of each pocket. This is the freshness baseline. The `<codename>` release pocket is not compared: its `Date` is the release date (for example `noble` shows April 2024) and never changes, so it says nothing about freshness.
7. **Probe** each candidate in parallel (`std::thread`): fetch the same two `InRelease` files from `<mirror>/dists/...` and parse `Date:`. Measure connect latency. Measure throughput on a larger sample (`Range: bytes=0-1048575` of `main/binary-<arch>/Packages.xz`), because a few KB of `InRelease` only measures TCP slow start. Redirects: follow at most 3, re-validate the final host and scheme; report mirrors that redirect to another host.
8. **Reject** unreachable mirrors, non-200 responses, and stale mirrors. `<codename>-updates` decides: a mirror is stale if its `Date` is older than the reference by more than `MAX_LAG` (24 hours). `<codename>-security` lag is shown for information only, because `security.ubuntu.com` is left untouched. A missing `-updates` pocket or unparsable `Date` rejects. `Date` is the signing time, not the sync time. If the reference host is unreachable (network error, 5xx), use the newest `Date` among candidates as baseline and print a warning; a `Date` more than `MAX_LAG` ahead of the median is treated as bogus and ignored, so one wrong clock cannot make every other mirror stale. Only the `<codename>-updates` reference decides whether the release left the archive; an unreadable `-security` reference is ignored (it is informational). If the reference answers 404 for the codename, the release is no longer on `archive.ubuntu.com` (it moved to `old-releases.ubuntu.com`, which has no mirrors): exit 4 with a message that names the codename and explains this; never use the newest-candidate baseline in that case. Measured on 2026-11-11 (`noble-updates`): of 10 Lithuanian and Norwegian mirrors, 6 matched the reference exactly and 4 were identical to each other but one signing generation behind (about 6 hours), so a 1-hour limit would reject 40% of good mirrors. Freshness also guards against replay of old indexes; apt still verifies signatures.
9. **Rank**: compare `-updates` `Date` exactly (no tolerance); lag is the reference `Date` minus the mirror `Date`, clamped to 0 because the reference is fetched first and a mirror may be newer. Primary key: lag ascending (mirrors with equal `Date` form one group). Within a group: throughput descending, then latency ascending; a mirror in the same AS as the server wins when its throughput is within 20% of the group's fastest mirror (band measured against that top speed, so it is not chained). A lagging but not stale mirror stays selectable and shows a warning. Accepted trade-off: a slow mirror with the exact reference `Date` ranks above a much faster one that is one signing generation (about 6 hours) behind. CLI output must show the actual mirror AS (or `unknown`) and same-AS status (`same`, `different`, or `unknown`) so users can understand and audit the AS tie-break; do not imply AS preference when lookup failed.
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

- Discovery, network access, parsing, rewrite planning, and user confirmation run unprivileged. Root is needed only for the final file operations. Do not implement self-reexec through `sudo` or another privileged helper until the privilege-boundary design passes D6e security audit. The privileged phase must not perform network access, invoke a shell, resolve commands through `PATH`, or consume unvalidated arbitrary paths. Never write without root.
- Source discovery and inventory are read-only. Scan `/etc/apt/sources.list` and regular `*.list` / `*.sources` entries under `/etc/apt/sources.list.d/`; do not follow symlinks. Report each inspected file and each skipped file/stanza/URI with its reason before mirror probing. If no eligible official archive URI exists, stop before reference/mirror probes; dry-run exits 0 and apply mode will exit 1. D7 extracts URI tokens and suites only; it performs no writes.
- Rewrite per URI token, not per file or line. Rewrite a URI when its host is `archive.ubuntu.com`, `*.archive.ubuntu.com`, or any host in the current mirror list (so a previously chosen mirror is recognized without any manifest). Never rewrite `security.ubuntu.com`, `ports.ubuntu.com`, PPAs or third-party hosts. A line or stanza whose suites include a `-security` suite is never rewritten to a probed mirror, even when its host is `archive.ubuntu.com`, because `-security` freshness is not checked; report each skipped entry and why. The only exception is the Pop!_OS split above, which keeps `-security` on the original mirror. On Ubuntu 24.04 and later security already lives in its own stanza (`security.ubuntu.com`), so no split is needed.
- deb822 (`/etc/apt/sources.list.d/*.sources`): a stanza's `URIs:` field may hold several URIs; D7 recognizes continuation lines, repeated fields, comments and stanza boundaries; rewrite only matching tokens in later milestones. Keep every other field (`Types`, `Suites`, `Components`, `Signed-By`, ...) and comments byte for byte. Do not touch stanzas with `Enabled: no`. Missing or malformed `URIs`/`Suites`, duplicate singleton fields, and unknown formats are reported and never guessed.
- Legacy (`/etc/apt/sources.list`, `*.list`): D7 recognizes `deb`/`deb-src` lines, bracketed options such as `[arch=... signed-by=...]`, comments and URI tokens; rewrite only the URI token in later milestones. Malformed lines and unsupported URI schemes are reported and never guessed.
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

## Privilege boundary and threat model

- Security goal: untrusted inputs must not let an attacker execute code as root or write outside the explicitly allowed apt paths through apt-scout.
- Treat CLI arguments/environment, apt source contents, network responses, mirror URLs, backup manifests, file metadata/races, and terminal input as untrusted.
- All network requests, DNS, parsing, ranking, rewrite planning, and confirmation happen without elevated privileges. Root is limited to a small final operation over a previously validated plan.
- The root path must use fixed executables/paths and a minimal controlled environment; it must not invoke a shell, use PATH lookup, or make network requests. Validate file type, ownership/mode policy, path containment, and symlink/TOCTOU behavior at the point of use; fail closed on any mismatch.
- Do not use privileged self-reexec of the current executable until D6e approves a concrete trust mechanism. Security review reduces risk but is not a guarantee of absolute immunity.
- Before D9 or any privileged write implementation, D6c and D6e must be complete. Every privileged-write PR needs a security-focused exact-diff review and disposable Ubuntu-container tests for path traversal, malformed manifests, symlinks/races, backup failures without partial writes, rollback, and rejection of paths outside `/etc/apt`. Never run privileged write tests on the developer host.

## Resilience

Third-party services change formats, add limits or become paid. The program must keep working when they do.

- Geolocation and AS lookups are optional. If country detection fails, the program asks for `--country`; it never silently guesses a worldwide list. If AS lookup fails, ranking proceeds without AS preference. Failures include source and reason.
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
- Docker integration tests for source discovery/apply/restore must use disposable Ubuntu containers and isolated `/etc/apt` fixtures. Never mount or mutate the developer host's `/etc/apt`; test rollback, manifest path validation, permissions, and byte-exact restore in the container.

## Non-functional

- Static musl binary. `ureq` with rustls, `ratatui` + `crossterm` for the TUI. No async, no OpenSSL.
- All output, errors and docs in English.
- Ship a comprehensive Linux man page at `man/apt-scout.1`, accessible as `man apt-scout`. It documents only implemented behavior and covers SYNOPSIS, options/modes, security/privacy, files, errors, exit status, practical command examples, and usage recipes. Install it in `.deb` as `/usr/share/man/man1/apt-scout.1` and include it in the release `.tar.gz` with installation instructions.
- Exit codes: 0 ok, 1 runtime error, 2 usage error, 3 not root, 4 unsupported distro or architecture.
- Milestones: (1) read-only distro, geo, mirror list, probe, freshness, rank, and CLI — complete (PRs #13–#16); (2) apt source discovery/rewrite, apply, backup, and restore — in progress (D7 inventory and D8 pure rewrite plan complete; D6c signature verification next); (3) TUI core flow and user laptop terminal inspection; (4) TUI `Insert`/`Delete`. The execution sequence, task status, dependencies, PRs, and milestone audit gates live in `ROADMAP.md`.
- Suggested layout: `main.rs`, `error.rs`, `distro.rs`, `geo.rs`, `mirrors.rs`, `probe.rs`, `fresh.rs`, `rank.rs`, `sources.rs` (pure string in/out), `apply.rs`, `tui/`. Keep `sources`, `rank`, `fresh` pure; HTTP is injected as a function so tests use canned responses.
- Test fixtures: Ubuntu 22.04 `sources.list`, 24.04 `ubuntu.sources`, multi-URI stanza, `Enabled: no`, comments, `deb-src`, arm64 ports, `InRelease`, mirror list HTML and text, corrupt cache. D7 tests read-only source inventory/parsers; D8 tests byte-preserving rewrite output and URI round trips. Later apply milestones add restore-equals-original, tempdir atomic write, and failed-backup-abort tests. TUI uses `ratatui` `TestBackend` snapshots.
- Parsing and ranking are pure functions, unit-tested on fixtures; no live network in tests.
- Security note: all geolocation and AS answers are untrusted input, validated, and influence ranking only. Canonical's lookup is plain HTTP; AS data comes from DNS. Nothing derived from them is ever written into apt sources.
- Freshness trust boundary: ranking may display and use an `InRelease` `Date` even when signature verification is `invalid` or `unavailable`; this can mis-rank a mirror but cannot authorize package installation. Every full `InRelease` used by probing is verified with `/usr/bin/gpgv --keyring /usr/share/keyrings/ubuntu-archive-keyring.gpg`, using no shell, no `PATH` lookup, a cleared environment, a private temporary directory, a 512 KiB verification cap, a 10-second process timeout, and a 1 MiB cap on captured status output. Missing/invalid `gpgv` or keyring, unsigned/changed/unknown-key data, oversize data, and execution errors are shown explicitly and are never `Verified`. Before `--apply` may use a ranked mirror, the selected mirror's `-updates` `InRelease` and the canonical reference used as freshness baseline must both be `Verified`; fallback baselines are acceptable only when computed from verified candidate dates. If verification is not available, write mode fails closed while read-only ranking still works. `.deb` packages must depend on `gpgv` and `ubuntu-keyring`; tarball instructions must name them.

## Testing

- Unit tests (`#[cfg(test)]`) cover pure logic on fixtures: `os-release`, Launchpad HTML, `mirrors.txt`, `InRelease` dates, freshness, ranking, read-only apt source inventory/parsers, and D8 source rewrite output/round trips. Saved fixtures come from real responses (the Launchpad page is ~316 KB; keep a trimmed copy with at least two countries and every status value seen).
- Docker integration tests run inside disposable real Ubuntu images; offline cases and live-network smoke cases are both included. Live cases require internet and are manual, never part of CI. The matrix covers every supported release; format measured on 2026-11-11:

  | Release | Codename | Sources format |
  | --- | --- | --- |
  | 20.04 | focal | legacy `/etc/apt/sources.list` (10 `deb` lines) |
  | 22.04 | jammy | legacy `/etc/apt/sources.list` |
  | 24.04 | noble | deb822 `/etc/apt/sources.list.d/ubuntu.sources` |
  | 26.04 | resolute | deb822 `/etc/apt/sources.list.d/ubuntu.sources` |

  Releases whose reference `InRelease` returns 404 are handled by the exit-4 rule in step 8. `focal-updates` was still receiving updates on 2026-11-11 (`Date` 2026-11-11), so freshness stays meaningful for 20.04; keep a focal fixture. `i386` is in scope but covered by fixtures only (no i386 image). The `amd64` images are used (`--platform linux/amd64`); on Apple Silicon they run under emulation, slower but equivalent. Current Docker tests cover distro detection, read-only inventory/early stop, and live mirror ranking. Later source-write milestones must test rewrite, backup, atomic write, `--restore` and error messages inside disposable containers with isolated `/etc/apt`. Network answers in automated tests are otherwise injected from fixtures.
- One script (`scripts/docker-test.sh`) runs the container tests on the developer's machine only; CI does not run Docker tests (CI builds and runs static checks). Run it before opening a PR that changes `.rs` files or build configuration, and state the result in the PR. TUI milestone additionally requires a laptop launch against safe fixture/mock data for user visual inspection, followed by runtime checks in local Ubuntu Docker emulation; visual acceptance requires explicit user confirmation.
- Pop!_OS is tested in an Ubuntu 22.04 container with the real `pop-default-settings` package downloaded from `apt.pop-os.org` (pinned by file checksum in the test fixtures, downloaded once and cached; the download is a test setup step, not a test of the live service): its `os-release` and the deb822 `system.sources` template from its `postinst` are the fixture. Linux Mint is covered by its documented `os-release` (`ID=linuxmint`, `VERSION_CODENAME=wilma`, `UBUNTU_CODENAME=noble`) in a container. Neither is a full installation, only the files this tool reads and writes.
- Live-network smoke tests (real Launchpad, geo sources) are manual, never part of CI, because they are rate limited and flaky.
- Man-page verification checks `man/apt-scout.1` formatting (for example with `mandoc -Tlint`) and confirms `man apt-scout` renders in a disposable Ubuntu container. Every example must match `--help` and the implemented CLI; do not document planned flags as available.

## Implementation status

Milestone 1 is complete (PRs #13–#16). Implemented and tested: Ubuntu-family/codename and architecture detection; Launchpad and `mirrors.ubuntu.com` candidate discovery; URL/DNS hygiene; optional country/AS detection and Team Cymru ranking; `-updates`/`-security` freshness; parallel probe and ranking; read-only `--country`, `--no-geo`, `--top`, `--https-only`, and `--list` flow. `--country` is required if country detection fails. Probing runs all candidates in parallel, so bandwidth is shared and throughput is relative.

Milestone 2 is in progress: read-only apt source inventory/parsers merged in PR #21; pure byte-preserving URI rewrite planning merged in PR #23. D8 does not write files. Signature verification (D6c), apply, backup, and restore are not implemented. Apt writes remain blocked until D6c and D6e are complete. Other planned work not yet implemented: regional top-up, RIPEstat fallback, geo/mirror cache, built-in snapshot, `--refresh`, TUI core, TUI `Insert`/`Delete`. See `ROADMAP.md` for ordered tasks and status.

## Out of scope (v1)

Debian, Fedora/other package managers, running `apt update`, scheduling, GUI, PPA mirrors.
