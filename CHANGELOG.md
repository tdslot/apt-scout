# Changelog

All notable changes to this project are documented here. Generated from Conventional Commits by the "Prepare release" workflow.

## 1.1.1 - 2026-11-15

### Fixed

- lift the accent off the red with weight, not a new hue (#82)

## 1.1.0 - 2026-11-15

### Added

- paint the report structure in Ubuntu orange (#78)

## 1.0.0 - 2026-11-15

### Added

- answer --version and name a command the reader can run (#63)
- reach the restore from the command line (#62)
- run a restore through the one writer, with no baseline (#60)
- re-derive a restore plan from the newest backup (#59)
- make the default report the decision and --verbose the proof (#53)
- add --any and widen once when a country has nothing usable (#52)
- show progress, colour the report and stop repeating lines (#51)
- add short aliases for the flags that exist (#49)
- print the run as one structured report (#47)
- write the best mirror from the CLI when the run is root (#43)
- add the write harness and the container matrix for S5-S17 (#39)
- back up, re-validate and atomically write the planned targets (#36)
- add the pure core of the privileged writer (#31)
- verify Ubuntu InRelease signatures (#25)
- plan byte-preserving apt source rewrites (#23)
- inventory apt source files without writes (#21)
- show mirror AS and tie-break status (#19)
- detect server country and rank mirrors by AS (#16)
- probe mirrors, rank by freshness and speed, add read-only --country run (#15)
- parse Launchpad and mirrors.ubuntu.com lists, validate candidate URLs (#14)
- detect Ubuntu family and codename from os-release, add docker integration tests (#13)

### Fixed

- bind a file removal to a restore plan (#64)
- recognize a mirror host from every country so switching country works (#48)
- check the static binary with grep, not ripgrep (#44)
- close the D6c P1 findings with a signed-region date and a verified-value gate (#29)
- validate country and architecture before network access (#18)
- changelog base tag, deb metadata and checksum file (#3)
