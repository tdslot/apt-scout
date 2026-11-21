# Errors and self-healing

Principle: the program solves its own problems when safe, and otherwise says exactly what happened,
where, why, and what to do.
Vague messages ("Unknown error", "Something went wrong", bare `No such file`) are forbidden.

- Every error message states: **what** failed, **which path/URL/value**, **why** (the underlying
  OS or HTTP error, kept verbatim), and **how to fix** it.
  Example:
  `Cannot write /var/backups/apt-scout/: permission denied (os error 13). The path is not writable even as root, which means a read-only mount or an immutable attribute; nothing was written.`
- Error types carry context at the failure point (path, URL, step name); never discard the source
  error.
  No `unwrap()` on IO or parsing.
- Self-heal automatically (and tell the user in one line what was done):
  - `/var/backups/apt-scout` is created by the writer and verified after creation to be a
    directory, owned by root, mode 0700, and not a symlink; a pre-existing path that fails any of
    these is a hard abort with nothing written.
  - Missing `/etc/os-release`: read `/usr/lib/os-release` instead.
    There is no `lsb_release` path and no flag that passes a codename or a distro (`--codename` and
    `--distro` are unknown options, exit 2), so both files unreadable is exit 1 with
    `Cannot read os-release (<both paths and their OS errors>). apt-scout reads /etc/os-release (or /usr/lib/os-release) to find the Ubuntu codename; run it on an Ubuntu-based Linux system.`
  - Missing `UBUNTU_CODENAME` and `VERSION_CODENAME`: exit 1 naming the file it read and saying
    the Ubuntu release cannot be determined, because the codename is not guessed from the apt
    sources and no flag passes one.
  - Apt sources at an unexpected location or format: search `/etc/apt/sources.list`,
    `/etc/apt/sources.list.d/*.{list,sources}`, and report every file inspected and why each was
    skipped or used.
  - Network failure: one attempt per source and no retry, cache or snapshot.
    The mirror list goes to Launchpad, then to `mirrors.ubuntu.com/<CC>.txt`, once each; a failure
    is reported with its source and its reason, and when every source fails the run prints
    `no mirror list is available:` followed by each source and its reason, then exits 1 without
    planning or writing anything.
    Geolocation and AS failures are `note:` lines and the run continues without them (`SPEC.md` resilience).
  - Interrupted write: temp files are cleaned up on the next run; the original file is never
    left half-written.
- There is no cache to heal, because v1 keeps none: no cache directory and no cache file exist, so
  the mirror list, the geo answers and the AS lookups are fetched afresh on every run and no stale
  copy is ever served.
- Do not self-heal when it is unsafe: never guess which apt file to overwrite, never continue
  after a failed backup, and self-healing never creates a file in `/etc/apt`.
  That last prohibition is about self-healing, not about planned writes: the only file apt-scout
  ever creates there is the planned `sources.list.d/system-security.sources` on Pop!_OS (see Apply),
  which comes from the validated plan and is opened with `create_new`.
  Stop and explain instead.
- No silent data-source fallbacks: whenever a fallback is used, print which one and why.
  Message text falling back to English is not a data-source fallback and is not announced (see
  Localization).
- Exit codes (see Non-functional) always accompany a message that explains the cause.
- Tests cover the message content for the common failures (missing dir, permission denied,
  unreachable network, unknown distro).
- Docker integration tests for source discovery/apply/restore must use disposable Ubuntu
  containers and isolated `/etc/apt` fixtures.
  Never mount or mutate the developer host's `/etc/apt`; test rollback, manifest path validation,
  permissions, and byte-exact restore in the container.
  Write tests cover: a non-root run writing nothing and exiting 3, a root write aborting before its
  first byte on a failed validation, symlinked targets and a symlinked `sources.list.d`,
  substitution between validation and write, a target whose recomputed digest does not match the
  bytes to be written, a restore whose manifest is stale, partial, or names a path outside
  `/etc/apt`, a read-only `/etc/apt`, backup failure aborting before any write, byte-exact restore
  of content/mode/owner, hostile `PATH`/`HOME`/`TMPDIR`, a pre-existing `system-security.sources`
  aborting the apply, and a write that still succeeds when `/usr/bin/gpgv` is removed after
  verification has already happened (test hook), proving the write does not re-verify.
