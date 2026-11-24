# Split Pop!_OS `-security` into its own sources file

Pop!_OS keeps all four suites — `<codename>`, `-updates`, `-security`, `-backports` — in one stanza of
`/etc/apt/sources.list.d/system.sources`.
A deb822 `URIs:` field applies to every suite in its stanza, so moving the suites to a chosen mirror while leaving
`-security` on `http://apt.pop-os.org/ubuntu` cannot be expressed inside that stanza.
apt-scout therefore keeps `system.sources` for the non-security suites on the new mirror and writes a second file,
`/etc/apt/sources.list.d/system-security.sources`, holding the `-security` suite on the original mirror.

The split is a separate *file*, not a second stanza in the same file: with the real repolib 2.2.2 package, two stanzas
sharing one `X-Repolib-Name` are merged on `load()`+`save()` — the first stanza silently gains the `-security` suite.
Separate files with different `X-Repolib-Name` values load and save unchanged.
The new stanza keeps `Types`, `Components`, `Enabled` and `Signed-By` from the original and takes the name `Pop_OS
Security`.

This is the only case in which apt-scout creates a file under `/etc/apt/sources.list.d`; the created file is recorded in
the backup manifest and removed by `--restore`.

**Considered options**: a second stanza in `system.sources` — rejected, repolib merges it back; leaving `-security` on
the new mirror — rejected, it would move a pocket the mirror list does not vouch for.

## What the Pop!_OS package set was verified to do

Read from the real `pop-default-settings` package (`/etc/pop-os/os-release`, its `postinst`): the
active source is `/etc/apt/sources.list.d/system.sources`, carrying
`URIs: http://apt.pop-os.org/ubuntu`, the four suites of the base release and
`X-Repolib-Default-Mirror: http://apt.pop-os.org/ubuntu`. That host is a true Ubuntu mirror: it reports
`Origin: Ubuntu` and the same `InRelease` `Date` as `archive.ubuntu.com`, which is why it is treated as an
official archive URI for Pop!_OS and for nothing else.

The vendor default stays in `X-Repolib-Default-Mirror` untouched, because that is the value restore needs and the
value `apt-manage` itself keeps. `repolib` may add `X-Repolib-ID: system` on save, which is harmless.

Two things make the split safe to leave behind: `pop-upgrade` and `apt-manage` can reset the URIs to the
vendor default at any time, which simply undoes the rewrite, and the package `postinst` only creates
`system.sources` when it is missing, so a package upgrade does not overwrite the rewritten file.

**Spec**: `docs/spec/platform.md`. **Aliases**: D8.
