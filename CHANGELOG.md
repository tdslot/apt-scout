# Changelog

All notable changes to this project are documented here. Generated from Conventional Commits by the "Prepare release" workflow.

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

### Documentation

- run the Milestone 2 audit and file what it found (#66)
- make the record check itself, and require a test that can fail (#65)
- record what #51, #53, #56 and #58 shipped (#61)
- make README the complete user document (#58)
- add the apt-scout manual and install it from the .deb (#57)
- record the report, the cross-country rewrite and the short aliases (#50)
- plan localization with Fluent, the locale set and the localized manuals (#45)
- record D11a and D25 and reconcile the record with the merged head (#46)
- record D9d completion (#42)
- close out D9 with the ledger, glossary and record fixes (#41)
- record D9c completion and unblock D9d and D10 (#40)
- record D9b completion and unblock D9c (#38)
- add the speed, waiting and phase-budget rules the run learned (#34)
- record the workflow rules the team runs by (#33)
- record D9a and complete the root module list (#32)
- record the privilege-boundary decisions and threat model (#28)
- record D6c completion and open D6f findings (#27)
- record D8 completion and security gates (#24)
- record D7 completion and unblock D8 (#22)
- record AS output completion and unblock source inventory (#20)
- establish spec-driven delivery roadmap (#17)
- support Pop!_OS system.sources based on the real package (#11)
- replace rate-limited geo APIs with DNS and Canonical sources, add resilience rules (#10)
- note Launchpad rows without http(s) links (#9)
- update spec with measured mirror facts and container test strategy (#8)
- use HTTPS geo sources for server location, RIPEstat fallback for AS (#6)
- require documentation updates in every change (#5)
- require subagent code review before merging PRs (#2)
