#!/usr/bin/env bash
# Build an installable .deb from the static binary scripts/docker-test.sh --build-only produces.
# Usage: scripts/build-deb.sh [version]
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  echo "Usage: scripts/build-deb.sh [version]"
  echo
  echo "  version  package version; defaults to the version in Cargo.toml"
  echo
  echo "Writes dist/apt-scout_<version>_amd64.deb from target/docker/apt-scout-plain."
  echo "That binary comes from scripts/docker-test.sh --build-only; this script never builds it."
  echo "Builds with dpkg-deb on the host, or in an ubuntu:24.04 container when the host has none."
}

die() { echo "error: $*" >&2; exit 1; }

# Every packaging field lives in one place: Cargo.toml. They are read, never repeated below, so a
# change to [package.metadata.deb] reaches the .deb this script builds.
metadata() { sed -n "s/^$1 = \"\(.*\)\"\$/\1/p" Cargo.toml | head -n 1; }
# A field dpkg-deb requires and the manifest must supply: its absence stops the build, it never
# falls back to a default here.
meta_required() {
  local value
  value=$(metadata "$1")
  [ -n "$value" ] || die "cannot read [package.metadata.deb] $1 from Cargo.toml"
  printf '%s\n' "$value"
}

case ${1:-} in
  -h|--help) usage; exit 0 ;;
  -*) echo "unknown option '$1'; try --help" >&2; exit 2 ;;
esac
if [ "$#" -gt 1 ]; then echo "too many arguments; try --help" >&2; exit 2; fi

version=${1:-}
if [ -z "$version" ]; then
  version=$(metadata version)
  [ -n "$version" ] || die "cannot read the version from Cargo.toml; pass it as the first argument"
fi
# Debian's version syntax, refused here instead of by dpkg-deb: a version starts with a digit (a tag
# v0.1.0 ships as 0.1.0), and it becomes part of the output file name, so only characters Debian
# allows may reach dpkg-deb or a container shell.
case $version in
  [0-9]*) ;;
  *) die "invalid version '$version': a Debian version starts with a digit, so a tag v0.1.0 is passed as 0.1.0" ;;
esac
case $version in
  *[!A-Za-z0-9.+~:-]*) die "invalid version '$version': use only letters, digits and . + ~ : -" ;;
esac

maintainer=$(meta_required maintainer)
depends=$(meta_required depends)
section=$(meta_required section)
priority=$(meta_required priority)
synopsis=$(metadata description)
[ -n "$synopsis" ] || die "cannot read the description from Cargo.toml"

binary=target/docker/apt-scout-plain
[ -f "$binary" ] || die "$binary is missing; build it with scripts/docker-test.sh --build-only"
[ -x "$binary" ] || die "$binary is not executable; rebuild it with scripts/docker-test.sh --build-only"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/apt-scout-deb.XXXXXX") || die "cannot create a staging directory"
trap 'rm -rf "$tmp"' EXIT
stage=$tmp/stage
out=$tmp/out
mkdir -p "$stage" "$out"
# mktemp makes the directory 0700, and that mode would become the archive's own "." entry.
chmod 0755 "$stage"

install -d "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/doc/apt-scout"
install -m 0755 "$binary" "$stage/usr/bin/apt-scout"
install -m 0644 README.md "$stage/usr/share/doc/apt-scout/README.md"
# Debian Policy 12.5 makes /usr/share/doc/<pkg>/copyright mandatory, and requires the licence text
# itself except for the licences Debian already ships under /usr/share/common-licenses. MIT is not
# one of those, so its text is copied verbatim out of the file the licence ships as; Apache-2.0 is,
# so that half refers to the common-licenses copy instead of duplicating it into the package.
copyright=$stage/usr/share/doc/apt-scout/copyright
cat > "$copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: apt-scout
Source: https://github.com/tdslot/apt-scout

Files: *
Copyright: 2025 tdslot
License: MIT or Apache-2.0

License: MIT
EOF
# One leading space marks a DEP-5 continuation line and " ." marks a blank one. The text is read
# from LICENSE-MIT, so the packaged copy and the repository copy cannot drift apart.
[ -s LICENSE-MIT ] || die "LICENSE-MIT is missing; the copyright file has to carry its text"
sed -e 's/^$/./' -e 's/^/ /' LICENSE-MIT >> "$copyright"
cat >> "$copyright" <<'EOF'

License: Apache-2.0
 On Debian systems the full text of the Apache License, version 2.0 is in
 /usr/share/common-licenses/Apache-2.0.

Comment: /usr/bin/apt-scout is a static binary built by the Rust toolchain against musl, so it
 has no libc6 dependency; when it runs it uses gpgv and ubuntu-keyring from the system.
EOF
chmod 0644 "$copyright"
# Debian Policy 12.7: a package that installs /usr/share/doc/apt-scout must install a changelog
# there. It is generated from the version this build resolved, so the repo keeps no second copy,
# and "gzip -n" keeps the file name and timestamp out of the archive. The name is changelog.gz and
# not changelog.Debian.gz because the version carries no Debian revision, which makes dpkg treat
# this as a native package; that is the changelog name Policy gives a native package.
changelog=$stage/usr/share/doc/apt-scout/changelog.gz
if [ -n "${SOURCE_DATE_EPOCH:-}" ]; then
  # The reproducible-build convention: with it set the entry is dated from that epoch instead of
  # from the build clock, so the changelog text is the same on every build. GNU date and BSD date
  # spell "an epoch" differently.
  stamp=$(date -u -R -d "@$SOURCE_DATE_EPOCH" 2>/dev/null || date -u -R -r "$SOURCE_DATE_EPOCH")
else
  stamp=$(date -u -R)
fi
{
  printf 'apt-scout (%s) unstable; urgency=medium\n' "$version"
  printf '\n'
  printf '  * Static musl build; runs against the system gpgv and ubuntu-keyring.\n'
  printf '\n'
  printf ' -- %s  %s\n' "$maintainer" "$stamp"
} | gzip -9n > "$changelog"
chmod 0644 "$changelog"
# The man page lands in a parallel change; package it when it is there and never fail without it.
if [ -f man/apt-scout.1 ]; then
  install -d "$stage/usr/share/man/man1"
  install -m 0644 man/apt-scout.1 "$stage/usr/share/man/man1/apt-scout.1"
fi

# Installed-Size is what dpkg reports: the bytes under usr, in KiB rounded up. DEBIAN is excluded
# because dpkg computes the field from the data archive alone.
installed_bytes=$(find "$stage/usr" -type f -exec wc -c {} + | awk '$2 != "total" { sum += $1 } END { print sum + 0 }')
installed_size=$(((installed_bytes + 1023) / 1024))

control="$stage/DEBIAN/control"
{
  printf 'Package: apt-scout\n'
  printf 'Version: %s\n' "$version"
  printf 'Architecture: amd64\n'
  printf 'Maintainer: %s\n' "$maintainer"
  printf 'Installed-Size: %s\n' "$installed_size"
  printf 'Depends: %s\n' "$depends"
  printf 'Section: %s\n' "$section"
  printf 'Priority: %s\n' "$priority"
  printf 'Description: %s\n' "$synopsis"
  printf ' Detects the server country and autonomous system, fetches the Launchpad mirror\n'
  printf ' list, probes freshness and speed, ranks the candidates, and rewrites the apt\n'
  printf ' sources to the best verified mirror after one confirmation. A root run backs up\n'
  printf ' every file it replaces under /var/backups/apt-scout/<id>/ with a manifest; a\n'
  printf ' non-root run only recommends and prints the sudo line, and --list never writes.\n'
} > "$control"
# dpkg-deb rejects a control file whose last byte is not a newline; check rather than assume.
[ -z "$(tail -c 1 "$control")" ] || die "the generated control file does not end with a newline"

name=apt-scout_${version}_amd64.deb
# The container never sees dist/: a package is built into the temp dir first, so a failed or
# interrupted build cannot leave a stale file in dist/ looking like its result.
if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --build --root-owner-group "$stage" "$out/$name" >/dev/null \
    || die "dpkg-deb could not build $name"
else
  command -v docker >/dev/null 2>&1 \
    || die "dpkg-deb is not installed and docker is not available; install either one to build a .deb"
  # The staging tree is mounted whole; the container is the only network use and writes only to
  # the temp output directory, which is thrown away after the .deb is moved into dist/.
  # $name travels as an argument, never inside the shell string, so it cannot split the command.
  docker run --rm --platform linux/amd64 \
    -v "$stage":/pkg:ro -v "$out":/out \
    ubuntu:24.04 sh -c 'dpkg-deb --build --root-owner-group /pkg "/out/$1"' _ "$name" >/dev/null \
    || die "the ubuntu:24.04 container could not build $name"
fi
[ -f "$out/$name" ] || die "dpkg-deb did not write $name"

if command -v dpkg-deb >/dev/null 2>&1; then
  info=$(dpkg-deb -I "$out/$name")
else
  info=$(docker run --rm --platform linux/amd64 -v "$out":/out:ro ubuntu:24.04 dpkg-deb -I "/out/$name")
fi
fields=$(printf '%s\n' "$info" | awk '/^ *(Package|Version|Architecture|Maintainer|Installed-Size|Depends|Section|Priority): / { sub(/^ +/, ""); print }')
[ -n "$fields" ] || die "dpkg-deb -I did not report the control fields of $name"
# Each field is reported, not just some of them: a filter that matches nothing must not pass as a check.
for field in Package Version Architecture Maintainer Installed-Size Depends Section Priority; do
  printf '%s\n' "$fields" | grep -q "^$field: " || die "dpkg-deb -I did not report $field for $name"
done
# Policy requires the two files this package generates (12.5 copyright, 12.7 changelog) and the
# README beside them, so the archive listing is checked for all three rather than assumed.
if command -v dpkg-deb >/dev/null 2>&1; then
  contents=$(dpkg-deb -c "$out/$name")
else
  contents=$(docker run --rm --platform linux/amd64 -v "$out":/out:ro ubuntu:24.04 dpkg-deb -c "/out/$name")
fi
[ -n "$contents" ] || die "dpkg-deb -c did not list the contents of $name"
for path in ./usr/share/doc/apt-scout/copyright \
            ./usr/share/doc/apt-scout/changelog.gz \
            ./usr/share/doc/apt-scout/README.md; do
  printf '%s\n' "$contents" | awk '{ print $NF }' | grep -qxF "$path" \
    || die "dpkg-deb -c does not list $path in $name"
done

mkdir -p dist
deb=dist/$name
mv -f "$out/$name" "$deb"

echo "built $deb"
echo "size: $(wc -c < "$deb" | tr -d ' ') bytes"
printf '%s\n' "$fields"
