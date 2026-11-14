# apt-scout

Find a fresh, fast Ubuntu archive mirror and point apt at it. `apt-scout` does what `nala fetch` does without installing nala: it loads the Launchpad mirror list, probes candidates for freshness, speed and signature status, ranks them, and either recommends one mirror or rewrites the apt sources to use it. One static binary, no Python and no runtime to install.

## What it is

A run reads `/etc/apt/sources.list` and the regular `*.list` and `*.sources` files under `/etc/apt/sources.list.d/`, and changes only the URI token of a line whose host and path the mirror list names. It asks once, copies every file it will replace into a backup, and then writes atomically. It never runs `apt update`, never touches `security.ubuntu.com`, a PPA, a third-party repository, `ports.ubuntu.com` or a `mirror://` line, and it does not work on Debian.

## Requirements

- Ubuntu 20.04, 22.04, 24.04 or 26.04, or a derivative such as Pop!_OS or Linux Mint: `ID` or `ID_LIKE` in `/etc/os-release` must name ubuntu. The container matrix runs those four releases, and a Debian image as the case that must be refused with exit status 4.
- `amd64` or `i386`. Every other architecture installs from `ports.ubuntu.com`, which has no mirror to choose from, so the run stops with exit status 4 and changes nothing.
- `gpgv` and `ubuntu-keyring` for the `-updates` `InRelease` signature check. The `.deb` depends on both. Without them a run still ranks mirrors, every `verified` cell reads `?`, and no write is allowed.
- Root, for the write only. Any other uid gets the same discovery and ranking, a recommendation, and the one `sudo` line to copy.

## Install

## Quick start

## How a run works

## What the output means

## Options

## Modes and exit codes

## Backups

`man apt-scout` documents every option and mode, the report, the files it touches and the rules a write follows; the `.deb` installs that page at `/usr/share/man/man1/apt-scout.1`.

## Network and privacy

## Troubleshooting

## Documentation

## Packaging

```sh
scripts/docker-test.sh --build-only   # leaves the static binary at target/docker/apt-scout-plain
./scripts/build-deb.sh                # writes dist/apt-scout_<version>_amd64.deb
```

`build-deb.sh` reads the version from `Cargo.toml` unless you pass one, packages the static binary with its README, a `copyright` file carrying the MIT licence text, a `changelog.gz` generated from that version, and the man page when `man/apt-scout.1` exists, then checks the result with `dpkg-deb -I` and `dpkg-deb -c`. It uses `dpkg-deb` when the host has it and otherwise builds in an `ubuntu:24.04` container, so a Mac needs Docker. The package's maintainer, its `gpgv` and `ubuntu-keyring` dependencies, its section and its priority are read from `[package.metadata.deb]` in `Cargo.toml`, so that block stays the one place they are declared; a missing one stops the build. The output is one file, `dist/apt-scout_<version>_amd64.deb`. `scripts/build-deb.sh --self-test` checks the Cargo.toml readers without packaging anything.

One package covers every supported Ubuntu release rather than one per release: the binary is linked against musl, so it has no `libc6` dependency and no glibc ABI to match, and `gpgv` and `ubuntu-keyring` keep the same names and paths from 20.04 to 26.04. The split is by architecture (`amd64` today, `arm64` later), never by release. The Docker matrix is the compatibility evidence, and it is as wide as this: two cases run on every supported release - the offline release-codename detection and the live ranked-table run that needs the network - on 20.04, 22.04, 24.04 and 26.04; the legacy-layout refusal runs on 22.04 and the Debian refusal on `debian:12`; every other case runs on the pinned 24.04 image. The script makes two builds and reports two digests, the plain binary mounted in the read-only cases and the harness binary in the write cases, and the same pair runs in every image.

## License
