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

**Spec**: `docs/spec/platform.md`. **Aliases**: D8.
