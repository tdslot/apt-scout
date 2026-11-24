# Platform support
- `PLAT-1` The run MUST support Ubuntu and its derivatives, detected from `/etc/os-release` `ID` /
  `ID_LIKE`.
  The codename is `UBUNTU_CODENAME` when present, otherwise `VERSION_CODENAME` (ADR-0008).
- `PLAT-2` The run MUST support Pop!_OS (`ID=pop`, `ID_LIKE="ubuntu debian"`, `UBUNTU_CODENAME` equals
  the Ubuntu base, for example `jammy`).
  Its sources are rewritten in place, except that the `-security` suite moves to a second file
  (`/etc/apt/sources.list.d/system-security.sources`, the one file the run creates there, recorded in the
  backup manifest and removed by `--restore`) (ADR-0001).
  The plan shows that split before anything is written, with the created file and its bytes named in the
  confirmation a run prints (Apply).
  The rewrite changes only the `URIs` of `system.sources` and leaves `X-Repolib-Default-Mirror` as it is,
  so restore has the vendor default to put back; System76's own repositories
  (`apt.pop-os.org/release`, `apt.pop-os.org/proprietary`, `pop-os-apps.sources`) are never touched (ADR-0001).
  `apt.pop-os.org/ubuntu` is treated as an official archive URI for Pop!_OS only.
  The created stanza takes the `X-Repolib-Name` `Pop_OS Security` and copies `Types`,
  `Components`, `Enabled` and `Signed-By` from the original.
- `PLAT-3` The run MUST accept the architectures `amd64` and `i386` only (from
  `dpkg --print-architecture`).
  The subprocess MUST run with the environment cleared and its stdin set to null, so no variable a user sets
  before `sudo` reaches it; the threat model rules on that inheritance (`docs/threat-model.md`, A2).
  The call runs in every run that selects a mirror, which excludes `--restore`.
  Other architectures (arm64, ...) use `ports.ubuntu.com`, which has no mirror choice: the run MUST
  print an explanation and make no changes.
