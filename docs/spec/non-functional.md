# Non-functional

- `NFR-1` Every release MUST ship one static musl binary: `ureq` with rustls, `fluent-templates` for the
  message catalogs, `ratatui` + `crossterm` for the TUI.
  The crate MUST use no async runtime and MUST NOT link OpenSSL.
- `NFR-2` English MUST be the default and the fallback for every message, help text, error and manual.
  A run in another locale changes display text only; Localization states the rules.
  Source, comments, commit messages and the project's own documentation MUST stay English.
- `NFR-3` Every release MUST ship a comprehensive Linux man page at `man/apt-scout.1`, accessible as
  `man apt-scout`.
  It MUST document only implemented behavior and MUST cover SYNOPSIS, options/modes, security/privacy,
  files, errors, exit status, practical command examples, and usage recipes.
  The `.deb` MUST install it as `/usr/share/man/man1/apt-scout.1` and the release `.tar.gz` MUST carry
  it with installation instructions.
  The English page is the source of truth; a localized page for a shipped locale is allowed under
  Localization.
- `NFR-4` The run MUST exit with these codes: 0 ok, 1 runtime error, 2 usage error, 3 the run would
  write and this uid may not, and 4 no usable distro, architecture or release —
  an unsupported distro or architecture, or a release that left `archive.ubuntu.com` for
  `old-releases.ubuntu.com` because the reference `<codename>-updates` answered 404.
  Exit 3 is every non-root run other than `--list` that reaches the write; a usage error (2) and a failure
  earlier in the run (1) keep their own code (ADR-0009).
  A root run that found no eligible or no rewritable source MUST write nothing and exit 1, as Apply
  states; `--list` MUST exit 0 whatever the uid and a non-root run MUST exit 3.
- `NFR-5` The delivery sequence MUST live in `ROADMAP.md` alone: the task order, task states,
  dependencies, merged pull requests and milestone audit gates are the ledger's, and this chapter
  states requirements only.
- `NFR-6` The crate SHOULD follow this layout: `main.rs`, `error.rs`, `distro.rs`, `geo.rs`, `mirrors.rs`,
  `probe.rs`, `fresh.rs`, `verify.rs`, `rank.rs`, `sources.rs` (pure string in/out), `apply.rs`,
  `l10n.rs` (locale tag from two injected values, the compiled catalog loader, message lookup,
  display width), `locales/<tag>/main.ftl` (one Fluent catalog per locale, `en` the source of truth),
  `tui/`.
  `sources`, `rank`, `fresh` and `l10n` MUST stay pure except the one environment read, and HTTP MUST
  be injected as a function so tests use canned responses.
- `NFR-7` (planned) The test suite MUST include these fixtures: Ubuntu 22.04 `sources.list`, 24.04
  `ubuntu.sources`, multi-URI stanza, `Enabled: no`, comments, `deb-src`, arm64 ports, `InRelease`,
  mirror list HTML and text.
  The read-only source inventory and parsers MUST be tested against them, as MUST the byte-preserving
  rewrite output and its URI round trips.
  Every later apply milestone MUST add its own cases: restore-equals-original, tempdir atomic write,
  and failed-backup-abort.
  The TUI MUST use `ratatui` `TestBackend` snapshots.
- `NFR-8` Parsing and ranking MUST be pure functions, unit-tested on fixtures, and a test MUST NOT use
  the live network.
- `NFR-9` The run MUST treat every geolocation and AS answer as untrusted input: it MUST validate it,
  and it MAY influence ranking only.
  Canonical's lookup is plain HTTP; AS data comes from DNS.
  The run MUST NOT write anything derived from them into apt sources.
- `NFR-10` Freshness trust boundary: ranking MAY display and use an `InRelease` `Date` even when signature
  verification is `invalid` or `unavailable` (ADR-0009); it can never authorize an
  installation.
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
  Ranking MAY use a `Date` whose status is `invalid` or `unavailable`, but only a `Verified` `Date`
  parsed inside the signed region is usable by a write.
  Before a write may use a ranked mirror, the selected mirror's `-updates` `InRelease` and the write
  baseline MUST both be `Verified`.
  Ranking has a separate baseline that ignores verification and MAY fall back to the newest
  candidate `Date`, verified or not.
  The write baseline MUST NOT ignore verification: it is the canonical reference when that
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
  the change that closed both halves before the writer was wired in. `.deb` packages MUST depend on
  `gpgv` and `ubuntu-keyring`; tarball instructions MUST name them.
