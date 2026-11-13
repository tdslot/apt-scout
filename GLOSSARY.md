# Glossary

**Mirror**: A server that hosts a copy of the official Ubuntu archive and is listed on Launchpad.

**Candidate**: A mirror selected for probing after country filtering and capping.

**Probe**: The timed requests made to one candidate: the `InRelease` files of `<codename>-updates` and `<codename>-security` (freshness) and a ranged `Packages.xz` sample (throughput), plus time to first byte of the `-updates` request.

**Reference**: The canonical `archive.ubuntu.com` `InRelease` files; the baseline for freshness.

**Lag**: How much older a mirror's `InRelease` `Date` is than the reference for the same pocket.

**Freshness**: A mirror is fresh when its `<codename>-updates` lag is within `MAX_LAG` (24 hours). The release pocket is not compared and `-security` lag is informational. Stale mirrors are rejected; lagging but fresh mirrors rank lower.

**verified** (signature status): The `<codename>-updates` `InRelease` carries a good OpenPGP signature from the Ubuntu archive keyring. The only status a write may trust.

**invalid** (signature status): The `InRelease` was readable but is unsigned, altered, or signed by an unknown key.

**unavailable** (signature status): Verification could not run: `gpgv` or the keyring is missing, the file is over the size limit, or the check timed out or failed to execute. Ranking still works; a write does not.

**Server AS**: The autonomous system of the machine running apt-scout, as reported by the first geolocation source that answers.

**Geo source**: One of the sources tried in order to learn the server's country (Canonical geoip, `ifconfig.co`, `ipinfo.io`, Cloudflare trace) or AS (Team Cymru DNS, RIPEstat).

**AS match**: A candidate whose AS equals the server AS. Wins ties within a speed band; it never overrides a clearly faster mirror.

**Mirror list source**: Where the mirror list came from: Launchpad (primary), `mirrors.ubuntu.com`, disk cache, or built-in snapshot.

**Read-only run**: A run that writes nothing: `--list`, whatever the uid, and every non-root run, which prints the `sudo` command instead and exits 3.

**Backup**: Timestamped copy of every apt sources file changed by a write, stored under `/var/backups/apt-scout/`.

**Manifest**: The record inside a backup that lists, for each changed apt source file, its original path, SHA-256 digest, mode and owner, so a restore re-derives what to write from the backup instead of being told.

**Plan**: The validated description of a pending apt change: the exact bytes of every file to write and the proof that they come from verified data. The run builds and validates it, it is never a file, and it never crosses a process boundary.

**Privileged phase**: The part of a run that holds root. It validates the plan against the state on disk and performs the file operations; nothing else runs there.

**Write proof**: The value that ties a mirrored `InRelease` date to a successful signature verification. It is the only thing that authorizes a write.

**Official archive URI**: An apt source URI pointing at `archive.ubuntu.com` or a host in the current mirror list. The only kind apt-scout rewrites.

**Recommended**: Badge on the best-ranked fresh mirror; it is pre-checked in the TUI.

**Primary mirror**: The first checked mirror in rank order; written as the active apt URI.

**Alternative mirror**: Any other checked mirror; written as a `# alternative:` comment. apt does not use it.

**TUI**: The interactive terminal interface for reviewing mirrors and choosing which to use.

**Zero-effort flow**: The default TUI path where pressing `Enter` at every step saves the recommended mirror.

**Custom mirror**: A mirror URL added by the user with `Insert`; probed and ranked like any other candidate.

**Vendor default mirror**: The `X-Repolib-Default-Mirror` value in a Pop!_OS `system.sources` stanza. Left unchanged so the vendor default stays available for restore.

**Locale**: The language a run prints its messages in, taken from the shell environment. It selects display text and nothing else.

**Message id**: The stable table key a message is addressed by, chosen for the message's role and never derived from its English text, so rewriting English never moves a translated entry.

**Message catalog**: Every message in one locale, one entry per message id. A run prints from the catalog it selected, or from the English entry when that catalog has none, so a message is either translated whole or printed in English.
