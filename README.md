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

One `.deb` serves every supported release. The binary is linked against musl, so it has no `libc6` dependency and no glibc ABI to match, and `gpgv` and `ubuntu-keyring` keep the same names and paths from 20.04 to 26.04. The split is by architecture (`amd64` today, `arm64` later), never by release: the Docker matrix over `ubuntu:20.04` through `ubuntu:26.04` is the compatibility evidence — two builds, two digests, the same pair green in each image.

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

`scripts/build-deb.sh` stages that static binary, this README and, when `man/apt-scout.1` exists, the manual page, then builds the archive with `dpkg-deb` — on the host, or in an `ubuntu:24.04` container when the host has no `dpkg-deb`. It reads the version, the maintainer, the synopsis and the dependencies from `Cargo.toml` unless you pass a version as its argument, carries the MIT licence text in `usr/share/doc/apt-scout/copyright`, generates a `changelog.gz` from that version, and needs Docker for the container path a Mac has to use. The published release is packaged instead by `.github/workflows/release.yml` with `cargo deb` from the same `[package.metadata.deb]` metadata: two builders for one layout, kept in step by hand, and `ROADMAP.md` tracks the difference.

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

The report is one block, printed once when the probing and the ranking are done:

- the identity line — `apt-scout <version> · Ubuntu <release> (<codename>) · country: <name> (<source>)`;
- `Sources`, the directory scanned, then one line per source file with `rewrite` or `skip`, the line number, the host and, when skipped, the reason;
- the context line, `mirrors:` naming the list source and `reference:` naming the freshness baseline;
- `Ranked`, with the legend and the table, and `Detail` in a `--verbose` run only: the facts behind each row;
- `Recommendation`, the chosen mirror with its reason, then `Planned change`, the diff a write would make or the note that nothing is written;
- `Rejected`, only when something was rejected: each class of reason with its count and the hosts.

The table's columns are named for what they measure:

| Column | Meaning |
| --- | --- |
| `behind` | How far the mirror's `<codename>-updates` package lists trail the freshest date the run knows, in minutes; `0m` means up to date. It is freshness, never network latency, and the 24-hour limit is applied to this number. |
| `speed` | Throughput measured on a 1 MiB range request, in MB/s. |
| `latency` | Round trip to the first byte of the mirror's `-updates` request; it is never freshness. |
| `verified` | The `-updates` `InRelease` signature: `yes` when `/usr/bin/gpgv` accepted it with the Ubuntu archive keyring, `no` when the file was readable but unsigned, altered or signed by an unknown key, `?` when the check could not run at all. Only `yes` may authorize a write. |
| `notes` | `redir:<host>` when the mirror answers with a redirect to another host; a host that does not fit is cut and ends in `~`. The column is left out when no shown row has a note. |

The default report is the decision and `--verbose` is the proof: `-v` adds each row's AS number and its `same`/`diff`/`unknown` match with the server, the server's AS on the context line, each row's country and resolved address, both pocket dates in full with the `-security` lag, the exact signature status, the redirect host uncut, and the full text of every rejection reason — and it changes no decision, no ordering and no byte written.

While it works, a run shows one line on stderr, rewritten in place and only when stderr is a terminal: `loading the mirror list`, `verifying the reference`, `probing <n> mirrors… <done>/<total>`, `ranking`, then a summary such as `done — 4 usable, 0 rejected in 3.1 s`. The report itself is never split by it.

Colour is display-only and terminal-only: it appears when stdout is a terminal and neither `NO_COLOR` (any value, the empty one included) nor `--no-color` is set, so a captured run prints the same words without the escapes. A write ends with its own short block: `backup:` the directory it created, `wrote` each file it replaced, and a `changed …` line naming the next step, `apt update`.

## Options

| Short | Long | Meaning |
| --- | --- | --- |
| `-c` | `--country <CC>` | Two-letter country code; always wins over detection. |
| `-a` | `--any` | Ignore the country: rank the mirrors the list names for every country together. Needs no country, so it also satisfies `--no-geo`. |
| `-n` | `--no-geo` | No geolocation and no AS lookups; requires `--country` or `--any`. |
| `-s` | `--https-only` | Consider only `https` mirrors. |
| `-t` | `--top <N>` | Rows to show; default 5, and the value must be positive. |
| `-l` | `--list` | The table and the change a write would make, and never a write. |
| `-y` | `--yes` | Skip the one confirmation before a write. |
| `-v` | `--verbose` | The proof behind every row, and the full text of every rejection reason. |
| | `--no-color` | No colour, whatever the terminal is. The one flag without a short form. |
| `-h` | `--help` | The built-in summary of the flags, the table and the exit codes. |

Short flags are never bundled: `-lyc NO` is an unknown option. `-r` and `-R` are reserved for `--restore` and `--refresh`; `--restore`, `--refresh` and `--tui` are not implemented and are rejected as unknown options, which is exit 2.

## Modes and exit codes

The uid decides whether a run writes; no flag makes it write and no answer to the prompt can make it write.

| Command | uid | Writes apt sources | Exit |
| --- | --- | --- | --- |
| `apt-scout` | non-root | No: the table, the recommendation and the `sudo apt-scout …` line. | 3 |
| `sudo apt-scout` | root | Yes: the same run, one confirmation (`--yes` skips it), then the write. | 0 |
| `apt-scout --list` | any | No, whatever the uid: the table and the planned change. | 0 |

| Status | Meaning |
| --- | --- |
| 0 | Success, including `--list`, and a root run that found the sources already pointing at the best mirror. |
| 1 | Refusal or failure: no mirror passed the freshness and reachability checks, no source URI can be rewritten, a backup or a write failed. Nothing was written. |
| 2 | Usage error: an unknown option, a country code that is not two letters or that Launchpad does not list, `--no-geo` without `--country` or `--any`, a `--top` that is not positive. Nothing was probed. |
| 3 | This run would write and this uid may not. The `sudo` line is on stdout. |
| 4 | Unsupported distribution or architecture, or a release that has left `archive.ubuntu.com`. Nothing was written. |

## Backups

<<<<<<< HEAD
`man apt-scout` documents every option and mode, the report, the files it touches and the rules a write follows; the `.deb` installs that page at `/usr/share/man/man1/apt-scout.1`.
=======
Every file a write replaces is copied first into `/var/backups/apt-scout/<id>/`, where `<id>` is the UTC timestamp of the run as `YYYYMMDDTHHMMSSZ`. The directory is created mode 0700 and verified to be a real directory, owned by root and not a symlink; each copied file keeps its own mode, its owner and its path relative to `/etc/apt/`, so `/etc/apt/sources.list` is at `<id>/sources.list` and `/etc/apt/sources.list.d/ubuntu.sources` is at `<id>/sources.list.d/ubuntu.sources`. If any copy fails, nothing is written.

`MANIFEST` in the same directory records what the run did. It is created exclusively, mode 0600 and owned by root, so a run neither appends to an older manifest nor overwrites one:

```
apt-scout-backup 1
id 20261112T203045Z
files 1
file sources.list replace <64 hex digits> 420 0 0
end 1
```

One line per file: its path under `/etc/apt/`, `replace` or `create`, the SHA-256 of the bytes, the mode in decimal (`420` is 0644), the uid and the gid. Both the uid and the gid are 0: a backup is only ever written by root, for files root owns.

To restore by hand, copy the file back from the backup directory, as root, so the owner and the mode survive:

```sh
sudo cp -p /var/backups/apt-scout/<id>/sources.list /etc/apt/sources.list
sudo apt update
```

Repeat for each file the manifest lists. `--restore` is not implemented yet — the flag is rejected as an unknown option, so the copy above is the whole restore path today.
>>>>>>> bd7703a (docs: close the README with backups, privacy and troubleshooting)

## Network and privacy

Country detection contacts Canonical geoip, `ifconfig.co`, `ipinfo.io` and the Cloudflare trace in that order until the country, the public IP and the AS are found. Team Cymru DNS provides the AS lookups, for the server and for candidate mirror hosts. Those services, and the configured DNS resolver, can observe the server's public IP, and the DNS queries carry mirror IP addresses during AS ranking. The mirror list comes from Launchpad over `https` or from `mirrors.ubuntu.com` over `http`; the reference release and any `http` candidate are fetched without TLS, which is what the signature check is for. Proxies are ignored, so the environment cannot route a request through one. The signature check runs `/usr/bin/gpgv` with a cleared environment and a private, empty homedir, so no user GnuPG configuration can influence the answer.

`--no-geo --country CC`, or `--no-geo --any`, skips geolocation and the AS queries, so no third party learns the server's address; the mirror list itself still needs the network. Nothing else is uploaded and no telemetry is collected. The local reads are the apt sources, `/etc/os-release`, `/etc/resolv.conf`, the Ubuntu archive keyring, and the two programs invoked at fixed paths, `/usr/bin/dpkg` and `/usr/bin/gpgv`.

## Troubleshooting

| What you see | What it means | What to do |
| --- | --- | --- |
| `no mirror passed the freshness and reachability checks` (exit 1) | Every candidate was stale, unreachable, answered non-200, or carried no usable `InRelease` date. | Run with `-v` for each rejection in full, and try `--any`, which ranks every country's mirrors from the start. |
| `No official Ubuntu apt source URI can be changed` (exit 1, or 3 for a non-root run) | No file holds a URI whose host and path the mirror list names: the sources are a PPA, a third-party repository, `mirror://`, or a host the list does not name. | `--list` prints every file with the reason it was skipped. Point the URIs back at `archive.ubuntu.com` and run again. Changing `--country` is safe: eligibility follows every country's Launchpad entries, so a mirror this tool wrote stays replaceable. |
| Every `verified` cell reads `?` | `gpgv` or the Ubuntu archive keyring is missing or unusable, so the check cannot run. | `sudo apt install gpgv ubuntu-keyring`. Ranking works meanwhile, but a write is refused while no `yes` exists. |
| `man apt-scout` prints "This system has been minimized…" | A minimized image ships a 320-byte stub at `/usr/bin/man` and keeps the real reader at `/usr/bin/man.REAL`; a normal install has no stub. | Run `/usr/bin/man.REAL apt-scout`. On a container image, `unminimize` then `apt-get install man-db` is the other order that works. |
| `answered 404: <codename> is no longer on archive.ubuntu.com` (exit 4) | The release has moved to `old-releases.ubuntu.com`, which has no mirrors to choose from. | Nothing to fix on this host: no mirror can serve that release, so the run stops before probing. |

## Documentation

<<<<<<< HEAD
## License

MIT OR Apache-2.0
