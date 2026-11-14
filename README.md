# apt-scout

Standalone Rust CLI for finding fresh, fast Ubuntu archive mirrors and pointing apt at them. A root run backs up every file it replaces under `/var/backups/apt-scout/<id>/` and rewrites the sources atomically after one confirmation (`--yes` skips it); a non-root run recommends the same mirror, prints the one `sudo apt-scout …` line and exits 3, and `--list` never writes.

## Usage

```sh
apt-scout --country LT
apt-scout --country LT --no-geo --top 5 --https-only
apt-scout --any --no-geo
```

Without `--country`, apt-scout attempts optional country detection. If every source fails, it asks for `--country`, or for `--any`, which needs no country at all. `--no-geo` disables country and AS lookups and requires one of those two answers. `--any` (`-a`) ranks the mirrors the list names for every country together, so the country is shown but constrains nothing; with no country the `<CC>.txt` fallback has no file to read, so only Launchpad can supply that list.

A run stays in its own country and widens once only when that country has nothing usable: it then adds every other country's mirrors from the list it has already loaded, prints one `note:` line saying why, and ranks them together. Widening needs Launchpad's list, so a run whose only list is the one-country `mirrors.ubuntu.com` fallback says it cannot widen and fails as before. A set that is not narrowed to one country is capped at the 30 fastest-declared mirrors, so an `--any` run probes 30 of about 560 and a widened run at most 60.

Before mirror probing, apt-scout prints a read-only inventory of apt source URIs and stops if none can use an official Ubuntu mirror. `--list` prints ranked mirrors; `--top N` limits results. Each mirror's `-updates` `InRelease` signature is checked with `/usr/bin/gpgv` and `/usr/share/keyrings/ubuntu-archive-keyring.gpg`, and the `verified` column reports it in one word: `yes` when the check passed, `no` when the file was readable but unsigned, altered, or signed by an unknown key, and `?` when the check could not run at all. `--verbose` adds the exact status behind that cell. Ranking still works when verification cannot run, but only a `yes` may authorize a write. The `.deb` package depends on `gpgv` and `ubuntu-keyring`; tarball users should install both packages.

## Network and privacy

Country detection contacts Canonical geoip, ifconfig.co, ipinfo.io, and Cloudflare trace in order until country, public IP, and AS are found. Team Cymru DNS provides AS lookups for the server and mirror hosts. These services or the configured DNS resolver can observe the server's public IP; DNS queries also include mirror IPs during AS ranking. Use `--no-geo --country CC` (or `--no-geo --any`) to skip geolocation and AS queries.

See [SPEC.md](SPEC.md) for implemented behavior and planned milestones. See [AGENTS.md](AGENTS.md) for contributor checks.

## Packaging

```sh
scripts/docker-test.sh --build-only   # leaves the static binary at target/docker/apt-scout-plain
./scripts/build-deb.sh                # writes dist/apt-scout_<version>_amd64.deb
```

`build-deb.sh` reads the version from `Cargo.toml` unless you pass one, packages the static binary with its README, a `copyright` file carrying the MIT licence text, a `changelog.gz` generated from that version, and the man page when `man/apt-scout.1` exists, then checks the result with `dpkg-deb -I` and `dpkg-deb -c`. It uses `dpkg-deb` when the host has it and otherwise builds in an `ubuntu:24.04` container, so a Mac needs Docker. The package's maintainer, its `gpgv` and `ubuntu-keyring` dependencies, its section and its priority are read from `[package.metadata.deb]` in `Cargo.toml`, so that block stays the one place they are declared; a missing one stops the build. The output is one file, `dist/apt-scout_<version>_amd64.deb`. `scripts/build-deb.sh --self-test` checks the Cargo.toml readers without packaging anything.

One package covers every supported Ubuntu release rather than one per release: the binary is linked against musl, so it has no `libc6` dependency and no glibc ABI to match, and `gpgv` and `ubuntu-keyring` keep the same names and paths from 20.04 to 26.04. The split is by architecture (`amd64` today, `arm64` later), never by release. The Docker matrix is the compatibility evidence, and it is as wide as this: the case that detects the release codename offline runs on 20.04, 22.04, 24.04 and 26.04, and every other case runs on a pinned image, 24.04 except the legacy-layout refusal on 22.04 and the Debian refusal on `debian:12`. The script makes two builds and reports two digests, the plain binary mounted in the read-only cases and the harness binary in the write cases, and the same pair runs in every image.

## License

MIT OR Apache-2.0
