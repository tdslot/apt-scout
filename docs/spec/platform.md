# Platform support
- `PLAT-1` The run MUST support Ubuntu and its derivatives, detected from `/etc/os-release` `ID` /
  `ID_LIKE`.
  The codename is `UBUNTU_CODENAME` when present, otherwise `VERSION_CODENAME`: Linux Mint reports
  `VERSION_CODENAME=wilma` but apt uses Ubuntu's `noble`.
- `PLAT-2` The run MUST support Pop!_OS (`ID=pop`, `ID_LIKE="ubuntu debian"`, `UBUNTU_CODENAME` equals
  the Ubuntu base, for example `jammy`).
  Verified from the real `pop-default-settings` package (`/etc/pop-os/os-release`, `postinst`): the
  active source is the deb822 file `/etc/apt/sources.list.d/system.sources` with
  `URIs: http://apt.pop-os.org/ubuntu`,
  `Suites: <codename> <codename>-security <codename>-updates <codename>-backports`, and
  `X-Repolib-Default-Mirror: http://apt.pop-os.org/ubuntu`. `apt.pop-os.org/ubuntu` is a true Ubuntu
  mirror (`Origin: Ubuntu`, same `InRelease` Date as `archive.ubuntu.com`).
  Rewrite rules for Pop!_OS: rewrite only the `URIs` of `system.sources`; leave
  `X-Repolib-Default-Mirror` unchanged (it is the vendor default kept for restore, as `apt-manage`
  treats it); never touch System76's own repositories (`apt.pop-os.org/release`,
  `apt.pop-os.org/proprietary`, `pop-os-apps.sources`). `apt.pop-os.org/ubuntu` is treated as an
  official archive URI for Pop!_OS only.
  Because Pop!_OS keeps `-security` in the same stanza as the other suites, the rewrite moves it:
  `system.sources` keeps the non-security suites with the new mirror, and a new file
  `/etc/apt/sources.list.d/system-security.sources` holds a stanza for the `-security` suite on the
  original mirror (this is the one case where apt-scout creates a file in `/etc/apt/sources.list.d`;
  it is recorded in the backup manifest and removed by `--restore`).
  The new stanza gets a different `X-Repolib-Name` (`Pop_OS Security`) and copies `Types`,
  `Components`, `Enabled` and `Signed-By` from the original.
  Verified in a container with the real repolib 2.2.2 package: two stanzas with the same
  `X-Repolib-Name` in one file are merged on `load()`+`save()` (the first stanza gains the
  `-security` suite), so splitting inside one file is unsafe; separate files with different names
  load and save unchanged.
  The pure D8 transformation returns both the rewritten `system.sources` content and the planned new
  `system-security.sources` content without filesystem IO; D9 owns file creation, backup and atomic
  write.
  The diff shows the split before anything is written. `repolib`/`apt-manage` may add
  `X-Repolib-ID: system` on save; that is harmless.
  Tools such as `pop-upgrade` or `apt-manage` can later reset the URIs to the vendor default; the
  rewrite is then simply undone, which is acceptable.
  Check whether the `pop-default-settings` `postinst` overwrites `system.sources` on package upgrade
  (it only creates the file when missing, so an upgrade does not overwrite it).
- `PLAT-3` The run MUST accept the architectures `amd64` and `i386` only (from
  `dpkg --print-architecture`).
  The subprocess MUST run with the environment cleared and its stdin set to null, so no variable a
  user sets before `sudo` reaches it; the call runs in every run that selects a mirror, which excludes
  `--restore`, and it is the root run that makes the inheritance a hazard.
  Other architectures (arm64, ...) use `ports.ubuntu.com`, which has no mirror choice: the run MUST
  print an explanation and make no changes.
