# Apply

- `APPLY-1` One code path MUST write, and it MUST be internal.
  Discovery, verification, rewrite planning and confirmation produce a plan; the plan is validated
  against the closed target enumeration and the current on-disk state; only then does the first byte
  change.
  The plan is never a file, never a message on stdin, and never a flag: the user does not type a
  step of it, and it does not cross a process boundary.
  It names files through a fixed enumeration and carries the selected mirror, the verification
  result that authorizes writing it, the exact bytes, mode, and owner of every file it was computed
  from, and the exact bytes to write.
  It carries no freshness baseline of its own: the caller establishes one from the canonical
  reference when that reference has a verified `-updates` `Date` read inside the signed region,
  otherwise from the newest candidate in this run that carries one, after the clock-skew sanity
  guard of `pipeline.md` step 8, and the writer holds the selected mirror's proof against it before the first
  byte, refusing outright when no verified baseline was supplied: the writer makes no network
  request and cannot establish one itself (ADR-0006).
  The write aborts, before its first byte, on any missing field, any mismatch with the file on disk,
  any digest it cannot recompute to the same value, any target that is not a regular file after
  resolution, and any plan it did not fully validate.
  The plan binds to the disk by comparing those bytes, not by a hash; every digest the manifest
  records is SHA-256 over the exact bytes it describes.
  The write performs no network request of its own, invokes no shell, resolves no command through
  `PATH`, and reads no environment variable.
  Never write without root.
  The confirmation prompt is a safety rail, not a security control: the guarantee is the validation,
  and `--yes` removes only the prompt.
  A `--restore` plan is authorized by the backup it was re-derived from instead of by a verified
  mirror: it carries no selected mirror and no baseline, the baseline rule above is a mirror's rule,
  and every other requirement in this bullet applies to it unchanged.
  Only a `--restore` plan MAY remove a file: a removal is the one effect a restore can prove from
  the manifest it re-derived the plan from, so a plan that would remove anything while authorized by
  a mirror is refused before its first byte.
  A replacement is legitimate under either authority; a creation is legitimate only under a mirror
  plan, because a restore plan carries no platform and a creation without one is refused.
- `APPLY-2` Source discovery and inventory MUST be read-only.
  Scan `/etc/apt/sources.list` and regular `*.list` / `*.sources` entries under
  `/etc/apt/sources.list.d/`; do not follow symlinks.
  Report each inspected file and each skipped file/stanza/URI with its reason before mirror probing.
  If no eligible official archive URI exists, stop before reference/mirror probes: `--list` exits 0,
  a non-root run prints the `sudo` line and exits 3, and a root run that was going to write exits 1.
  D7 extracts URI tokens and suites only; it performs no writes.
- `APPLY-3` The run MUST rewrite per URI token, not per file or line.
  Rewrite a URI when its host is `archive.ubuntu.com`, `*.archive.ubuntu.com`, or a host and path
  the Launchpad mirror list names in any country, not only the run's own country, so a mirror this
  tool wrote stays rewritable after the operator changes country: otherwise a choice the tool made
  can be undone only by hand, which the run that made it MUST NOT leave behind.
  Membership answers recognizability, not trust: it makes an existing URI eligible for replacement
  and nothing else — the replacement is still the selected candidate, validated and with its
  `-updates` `InRelease` verified per probe, and the list source is trusted no further than it
  already was.
  Eligibility is by the listed URL's host and path, not by the host alone: an on-disk URI token is
  eligible only when its host matches a listed mirror's host and its path begins with that listed
  entry's path, compared by whole segments, so a listed `/ubuntu` admits `/ubuntu/` and
  `/ubuntu/dists/...` but not `/ubuntufoo`.
  A mirror host usually carries several trees, and the tool may replace only the Ubuntu tree:
  `deb http://ftp.uni-stuttgart.de/debian trixie main` is left alone even when that host is
  listed as an Ubuntu mirror at `/pub/linux/debian/ubuntu/`.
  A listed entry whose path is `/` admits every path on that host, because the list names no
  narrower tree there and the tool cannot tell the trees apart; that is acknowledged rather than
  hidden, and it is why the canonical archive and its `*.archive.ubuntu.com` names stay recognized
  at any path.
  A dot segment is never eligible and never enters the set: a token whose path carries `.`, `..`, or
  the `%2e` spelling of either is left unchanged with a reason naming the segment, and a listed
  entry whose path carries one is dropped and reported, so neither the any-path rule above nor a
  listed prefix can admit a path that traversal would resolve elsewhere.
  The test is lexical and reads exactly one spelling layer: a segment counts as `.` or `..` when
  `%2e`, in either case, is taken as a dot, no other escape is decoded, and no segment is resolved,
  so `%252e` is a literal segment name and `...` is an ordinary segment.
  The port is not part of the comparison, as before. `--https-only` does not narrow what is
  recognizable: a mirror written over `http` stays recognizable in a later `--https-only` run.
  Every entry of every list source passes the same candidate URL validation before it may enter the
  set, the Launchpad page and the `mirrors.ubuntu.com` fallback alike, and an entry that fails is
  reported and left out, never silently trusted.
  Launchpad is the only list that names every country; `mirrors.ubuntu.com` is read one country at a
  time (`<CC>.txt`) and its `mirrors.txt` names only the canonical archive, so outside Launchpad the
  eligible hosts and paths are the ones in the file the run did load and an entry outside it is
  reported as skipped rather than rewritten.
  The set therefore fails closed in the only direction open to it, shrinking and never widening, and
  when no URI is eligible the run stops under the no-eligible-URI rule and touches nothing.
  A path prefix is a recognition rule: when a host is listed at a path other than the one an on-disk
  URI carries, that URI is reported as skipped with its reason and left unchanged, not forced through
  (ADR-0006).
  Never rewrite `security.ubuntu.com`, `ports.ubuntu.com`, `ppa.launchpad.net`,
  `ppa.launchpadcontent.net`, or any host under `launchpad.net` or `launchpadcontent.net`: that
  refusal list is explicit and is checked before any membership test, whatever a list names.
  Naming those hosts is deliberate: the refusal is a decision the code makes, not one the data makes
  for it (ADR-0006).
  A line or stanza whose suites include a `-security` suite is never rewritten to a probed mirror,
  even when its host is `archive.ubuntu.com`: `-security` freshness is never checked (decision 0003). Report
  each skipped entry and why.
  The only exception is the Pop!_OS split above, which keeps `-security` on the original mirror.
  On Ubuntu 24.04 and later security already lives in its own stanza (`security.ubuntu.com`), so no
  split is needed.
- `APPLY-4` For deb822 (`/etc/apt/sources.list.d/*.sources`) the run MUST read a stanza's `URIs:` field
  as one or more URIs, recognize continuation lines, repeated fields, comments and stanza
  boundaries, and rewrite only the matching tokens.
  Keep every other field (`Types`, `Suites`, `Components`, `Signed-By`, ...) and comments byte for
  byte.
  The run MUST NOT touch stanzas with `Enabled: no`.
  Missing or malformed `URIs`/`Suites`, duplicate singleton fields, and unknown formats MUST be
  reported and never guessed.
- `APPLY-5` For legacy sources (`/etc/apt/sources.list`, `*.list`) the run MUST recognize `deb`/`deb-src`
  lines, bracketed options such as `[arch=... signed-by=...]`, comments and URI tokens, and MUST
  rewrite only the URI token.
  Malformed lines and unsupported URI schemes MUST be reported and never guessed.
- `APPLY-6` `mirror://` and `mirror+file:` URIs (older installs) MUST be left unchanged and reported
  with an explanation.
- `APPLY-7` (planned) In the TUI the first checked mirror in rank order MUST become the active URI.
  Other checked mirrors MUST be written as commented lines `# alternative: <uri>` above the changed
  line (ADR-0006).
- `APPLY-8` Before writing, the run MUST check that the target is a regular file (do not follow
  symlinks), MUST keep mode and owner, and MUST leave no half-written file.
- `APPLY-9` File creation MUST be planned, never incidental: the planned
  `/etc/apt/sources.list.d/system-security.sources` is created with `create_new` and mode 0600, and
  `EEXIST` MUST abort the whole apply with nothing written.
  A planned write that creates a file MUST be accepted only when the platform is Pop!_OS and the
  target is exactly `sources.list.d/system-security.sources`; a plan that creates any other file, or
  replaces a file that does not exist, MUST abort before the first open.
- `APPLY-10` The run MUST back up every file to be changed to `/var/backups/apt-scout/<UTC timestamp>/`,
  directory mode 0700, and MUST refuse a symlinked directory.
  It MUST write a manifest of original paths.
  It MUST abort the whole apply if any backup fails.
- `APPLY-11` The write MUST be atomic: temp file in the same directory, fsync, rename.
- `RESTORE-1` `--restore` MUST copy the newest backup back, in one root run: it reads
  `/var/backups/apt-scout/<backup id>/MANIFEST` itself and aborts before parsing a byte unless that
  path is a regular file, owned by root, mode 0600, and not a symlink.
  It then MUST re-derive the target set and every per-target digest from the manifest itself and
  MUST validate them: every entry matched to the closed target enumeration, every path under
  `/etc/apt/`, a regular file after resolution, and every recorded digest, mode and owner matching
  the backup copy it names, refusing the whole restore on the first disagreement; a file the run
  created has no backup copy, so its record carries the digest, mode and owner the run created it
  with, and the restore refuses to remove it unless the file on disk still matches.
  The manifest is the source of truth for what is written, not a list of files the run already
  chose, so a stale or partial manifest cannot apply a subset of the backup. `--restore` does not
  depend on the rewrite logic, and it reaches the writer through the same validation as an apply.
  It is executed by that same writer, through the one entry point the writer has: the restore engine
  re-derives the plan and hands it over with no baseline, so a restore adds no second write path and
  no write code of its own.
  The newest backup is the greatest id under `/var/backups/apt-scout/`, because the ids are
  zero-padded UTC timestamps and their string order is their age; it is the only backup a run
  considers, so a restore whose newest manifest or copy is refused fails as a whole and never falls
  back to an older backup, and a backup root that holds no id, or an id whose directory is not a
  plain directory, is a failure that writes nothing and exits 1.
  A restore is itself a write and backs up every file it replaces or removes under its own id,
  exactly as an apply does, so after one restore the newest backup is that restore's; the file it
  removed is one of them, recorded there as a file to put back with the bytes it held and with a
  copy beside it. What refuses a second restore from that backup is therefore the record-versus-disk
  check: the record says replace and the target is gone (ADR-0006).
- `APPLY-12` A custom URL (`Insert`) MUST use the scheme `http` or `https` only, and the run MUST reject
  userinfo, whitespace, `file:`, and loopback/private/link-local addresses; it MUST probe the URL
  like any candidate and MUST show a warning before apt is pointed at a user-supplied host.
- `APPLY-13` The run MUST say so before any probing when no URI is rewritable (for example arm64, or a
  system already using a third-party mirror): scan the sources first, print every file inspected and
  why each entry was skipped.
  `--list` exits 0, a non-root run prints the `sudo` line and exits 3, and a root run that
  was going to write exits 1.
  The run MUST NOT probe 30 mirrors for nothing.
- `APPLY-14` Candidate hygiene: every candidate from any list source MUST be `http`/`https`, have no
  userinfo, and not resolve to a private, loopback or link-local address: the list is untrusted data
  fetched over plain HTTP (ADR-0006).
  Results from the `mirrors.ubuntu.com` fallback MUST be marked unverified in the output.
- `APPLY-15` After an apply the run MUST suggest `sudo apt update` and MUST NOT run it.
  The run that wrote was a root run under `sudo` and has exited by the time the sentence is read, so
  a hint that named a bare `apt update` would tell the reader to run, from their own shell, a
  command that fails there.
  The sentence is written for that common case: a shell that is already root and has no `sudo`
  installed is not served by it, and telling the two apart would mean deciding a hint from the
  environment, which this spec does not allow as a decision input.
