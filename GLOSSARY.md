# Glossary

**Mirror**: A server that hosts a copy of the official Ubuntu archive and is listed on Launchpad.

**Candidate**: A mirror selected for probing after country filtering and capping.

**Probe**: The timed requests made to one candidate: the `InRelease` files of `<codename>-updates` and `<codename>-security` (freshness) and a ranged `Packages.xz` sample (throughput), plus connect latency.

**Reference**: The canonical `archive.ubuntu.com` `InRelease` files; the baseline for freshness.

**Lag**: How much older a mirror's `InRelease` `Date` is than the reference for the same pocket.

**Freshness**: A mirror is fresh when its `<codename>-updates` lag is within `MAX_LAG` (24 hours). The release pocket is not compared and `-security` lag is informational. Stale mirrors are rejected; lagging but fresh mirrors rank lower.

**Server AS**: The autonomous system of the machine running apt-scout, as reported by the first geolocation source that answers.

**Geo source**: One of the sources tried in order to learn the server's country (Canonical geoip, `ifconfig.co`, `ipinfo.io`, Cloudflare trace) or AS (Team Cymru DNS, RIPEstat).

**AS match**: A candidate whose AS equals the server AS. Wins ties within a speed band; it never overrides a clearly faster mirror.

**Mirror list source**: Where the mirror list came from: Launchpad (primary), `mirrors.ubuntu.com`, disk cache, or built-in snapshot.

**Dry-run**: Mode without TUI or `--apply`. Shows the ranking and the planned change; writes nothing.

**Backup**: Timestamped copy of every apt sources file changed by `--apply`, stored under `/var/backups/apt-scout/`.

**Official archive URI**: An apt source URI pointing at `archive.ubuntu.com` or a host in the current mirror list. The only kind apt-scout rewrites.

**Recommended**: Badge on the best-ranked fresh mirror; it is pre-checked in the TUI.

**Primary mirror**: The first checked mirror in rank order; written as the active apt URI.

**Alternative mirror**: Any other checked mirror; written as a `# alternative:` comment. apt does not use it.

**TUI**: The interactive terminal interface for reviewing mirrors and choosing which to use.

**Zero-effort flow**: The default TUI path where pressing `Enter` at every step saves the recommended mirror.

**Custom mirror**: A mirror URL added by the user with `Insert`; probed and ranked like any other candidate.
