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

The `.deb` is the short path:

```sh
sudo apt install ./apt-scout_<version>_amd64.deb
```

It installs `/usr/bin/apt-scout` and the manual page at `/usr/share/man/man1/apt-scout.1`, so `man apt-scout` resolves, and it pulls in `gpgv` and `ubuntu-keyring`.

One `.deb` serves every supported release. The binary is linked against musl, so it has no `libc6` dependency and no glibc ABI to match, and `gpgv` and `ubuntu-keyring` keep the same names and paths from 20.04 to 26.04. The split is by architecture (`amd64` today, `arm64` later), never by release: the Docker matrix over `ubuntu:20.04` through `ubuntu:26.04` is the compatibility evidence — one build, one digest, the same cases green in each image.

The release tarball holds the binary, the two licences and this file:

```sh
tar -xzf apt-scout-v<version>-x86_64-unknown-linux-musl.tar.gz
sudo install -m 0755 apt-scout-v<version>-x86_64-unknown-linux-musl/apt-scout /usr/local/bin/
sudo apt install gpgv ubuntu-keyring
```

Install the two packages yourself with the tarball: nothing declares them for you, and without them no write is allowed.

From source, with a stable Rust toolchain (edition 2024):

```sh
cargo build --release --locked            # target/release/apt-scout, linked against the host
scripts/docker-test.sh --build-only       # target/docker/apt-scout-plain, static amd64
scripts/build-deb.sh                      # dist/apt-scout_<version>_amd64.deb
```

`scripts/build-deb.sh` stages that static binary, this README and, when `man/apt-scout.1` exists, the manual page, then builds the archive with `dpkg-deb` — on the host, or in an `ubuntu:24.04` container when the host has no `dpkg-deb`. It reads the version, the maintainer and the synopsis from `Cargo.toml` unless you pass a version as its argument, and it needs Docker for the container path a Mac has to use. The published release is packaged instead by `.github/workflows/release.yml` with `cargo deb` from the same `[package.metadata.deb]` metadata: two builders for one layout, kept in step by hand, and `ROADMAP.md` tracks the difference.

## Quick start

```sh
apt-scout                                  # preview: the ranked table and a recommendation, nothing written
apt-scout --list --verbose                 # the same, plus the proof behind every row
sudo apt-scout                            # back up, ask once, then rewrite the sources
sudo apt-scout --yes                      # unattended: no prompt
sudo apt-scout --country DE               # choose the country instead of detecting it
sudo apt-scout --any                      # rank every country's mirrors together
```

A non-root run never writes. It prints the recommendation and one line, `sudo apt-scout` with the flags of that run, and exits 3, so the preview and the write are the same command with `sudo` in front. `--list` is the exception: it writes nothing for any uid and exits 0.

## How a run works

1. Read `/etc/os-release` for the Ubuntu family and the codename, and `/usr/bin/dpkg --print-architecture` for the architecture.
2. Detect the server's country and AS from optional third-party sources, unless `--no-geo`. If no country can be found, the run asks for `--country`, or for `--any`, which needs no country at all.
3. Load the mirror list: Launchpad first, then `mirrors.ubuntu.com/<CC>.txt` for the country. The first valid answer wins and the report names the source it used.
4. Pick candidates: the run's own country, or every country when `--any` is given; drop duplicates and, with `--https-only`, the `http` entries; cap the set at 30 — the first 30 the list names for a country run, the 30 with the fastest declared link speed for `--any`. If every candidate in the run's own country is unusable, widen once to every other country's mirrors from the list already loaded and say so in one `note:` line; a list that names only one country cannot widen.
5. Read the reference: the `Date` and the signature status of `<codename>-updates/InRelease` on `archive.ubuntu.com`. That is the freshness baseline a mirror is measured against.
6. Probe each candidate in parallel: its `-updates` `InRelease` in full for the date and the signature, a cheap read of its `-security` `InRelease` for information, and a 1 MiB sample of `Packages.xz` for throughput; time to first byte is the latency.
7. Reject what cannot be used: unreachable hosts, non-200 answers, a missing or unparsable `Date`, a sample that is not an xz file, and a mirror more than 24 hours behind the baseline.
8. Rank: smallest lag first, then throughput, then latency, with a same-AS mirror within 20% of its group's fastest taken first.
9. Write or recommend. A root run builds the plan, confirms once, backs up and writes; any other uid gets the recommendation and the `sudo` line.

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
