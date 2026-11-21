# Non-functional

- Static musl binary. `ureq` with rustls, `fluent-templates` for the message catalogs, `ratatui` +
  `crossterm` for the TUI.
  No async, no OpenSSL.
- English is the default and the fallback for every message, help text, error and manual.
  A run in another locale changes display text only; Localization states the rules.
  Source, comments, commit messages and the project's own documentation stay English.
- Ship a comprehensive Linux man page at `man/apt-scout.1`, accessible as `man apt-scout`.
  It documents only implemented behavior and covers SYNOPSIS, options/modes, security/privacy,
  files, errors, exit status, practical command examples, and usage recipes.
  Install it in `.deb` as `/usr/share/man/man1/apt-scout.1` and include it in the release `.tar.gz`
  with installation instructions.
  The English page is the source of truth; a localized page for a shipped locale is allowed under
  Localization.
- Exit codes: 0 ok, 1 runtime error, 2 usage error, 3 the run would write and this uid may not —
  every non-root run other than `--list` that reaches the write, which prints the command to run
  instead; a usage error (2) and a failure earlier in the run (1) keep their own code; 4 unsupported
  distro or architecture, or the release left `archive.ubuntu.com` for `old-releases.ubuntu.com`
  because the reference `<codename>-updates` answered 404.
  A root run that found no eligible or no rewritable source writes nothing and exits 1, as Apply
  states; `--list` exits 0 whatever the uid and a non-root run exits 3.
- Milestones: (1) read-only distro, geo, mirror list, probe, freshness, rank, and CLI — complete
  (PRs #13–#16); (2) apt source discovery/rewrite, apply, backup, and restore — in progress (D7
  inventory and D8 pure rewrite plan complete; D6c signature verification and its D6f follow-up
  merged as PRs #25 and #29); (3) TUI core flow and user laptop terminal inspection; (4) TUI
  `Insert`/`Delete`; (5) localization — the message catalogs, the shipped locale set and the
  localized manuals.
  The execution sequence, task status, dependencies, PRs, and milestone audit gates live in
  `ROADMAP.md`.
- Suggested layout: `main.rs`, `error.rs`, `distro.rs`, `geo.rs`, `mirrors.rs`, `probe.rs`,
  `fresh.rs`, `verify.rs`, `rank.rs`, `sources.rs` (pure string in/out), `apply.rs`, `l10n.rs`
  (locale tag from two injected values, the compiled catalog loader, message lookup, display width),
  `locales/<tag>/main.ftl` (one Fluent catalog per locale, `en` the source of truth), `tui/`.
  Keep `sources`, `rank`, `fresh`, `l10n` pure except the one environment read; HTTP is injected as
  a function so tests use canned responses.
- Test fixtures: Ubuntu 22.04 `sources.list`, 24.04 `ubuntu.sources`, multi-URI stanza,
  `Enabled: no`, comments, `deb-src`, arm64 ports, `InRelease`, mirror list HTML and text.
  D7 tests read-only source inventory/parsers; D8 tests byte-preserving rewrite output and URI round
  trips.
  Later apply milestones add restore-equals-original, tempdir atomic write, and failed-backup-abort
  tests.
  TUI uses `ratatui` `TestBackend` snapshots.
- Parsing and ranking are pure functions, unit-tested on fixtures; no live network in tests.
- Security note: all geolocation and AS answers are untrusted input, validated, and influence
  ranking only.
  Canonical's lookup is plain HTTP; AS data comes from DNS.
  Nothing derived from them is ever written into apt sources.
- Freshness trust boundary: ranking may display and use an `InRelease` `Date` even when signature
  verification is `invalid` or `unavailable`; this can mis-rank a mirror but cannot authorize
  package installation.
  Every `-updates` `InRelease` used by probing is verified with
  `/usr/bin/gpgv --keyring /usr/share/keyrings/ubuntu-archive-keyring.gpg`, using no shell, no
  `PATH` lookup, a cleared environment, a private 0700 temporary directory that is also `gpgv`'s
  `--homedir` (so no user GnuPG configuration or default home can influence the result), a 10-second
  process timeout, and a 1 MiB cap on captured status output.
  The download cap is the verifier's own 512 KiB cap, so an oversized file is rejected at download
  instead of being reported as an unverifiable but rankable mirror.
  The informational `-security` pocket is read as a ranged head of 4 KiB and is never verified,
  because nothing consumes its signature.
  Missing/invalid `gpgv` or keyring, unsigned/changed/unknown-key data, oversize data, and execution
  errors are shown explicitly and are never `Verified`.
  Only a `Date` from inside the cleartext-signed region (`pipeline.md` step 8) is ever trusted: bytes before
  `-----BEGIN PGP SIGNED MESSAGE-----` or after `-----BEGIN PGP SIGNATURE-----` are outside what
  `gpgv` signs, so a `Date` read there is shown and ranked, is never a verified date and never
  authorizes a write.
  Ranking may use a `Date` whose status is `invalid` or `unavailable`, but only a `Verified` `Date`
  parsed inside the signed region is usable by a write.
  Before a write may use a ranked mirror, the selected mirror's `-updates` `InRelease` and the write
  baseline must both be `Verified`.
  Ranking has a separate baseline that ignores verification and may fall back to the newest
  candidate `Date`, verified or not.
  The write baseline may not ignore verification: it is the canonical reference when that
  reference's own `verified_updates` field is `Some`, and otherwise the newest candidate whose own
  `verified_updates` field is `Some`; when neither the canonical reference nor any candidate yields
  one, a write fails closed.
  Trusted freshness dates are represented by a value that only a `Verified` status can construct
  (`VerifiedDate`), and the probe result, the reference and the ranked entry each expose it, so a
  write path takes that value and never a bare `Date`.
  If verification is not available, write mode fails closed while read-only ranking still works.
  `VerifiedDate` protects the date half only: it wraps a `fresh::SignedDate`, which only the strict
  in-region parser builds, so no write path can be handed a `Date` read outside the signed region.
  The status half is protected the same way: `SignatureStatus::Verified` carries a `VerifiedProof`
  whose single field is private and which only the verifier's `GOODSIG`+`VALIDSIG`+exit-0 path can
  mint, and that proof carries the date it read inside the region it verified — so a caller can
  neither forge the status nor pair its own date with a genuine proof. `ROADMAP.md` task D25 records
  the change that closed both halves before the writer was wired in. `.deb` packages must depend on
  `gpgv` and `ubuntu-keyring`; tarball instructions must name them.
