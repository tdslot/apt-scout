#!/usr/bin/env bash
# Build an installable .deb from a static binary: by default the one scripts/docker-test.sh
# --build-only produces, or the file --binary names.
# Usage: scripts/build-deb.sh [--binary <path>] [version]
set -euo pipefail
# This script's own path, absolute, resolved before the cd below: "$0" is relative to the caller's
# working directory, so `bash ../apt-scout/scripts/build-deb.sh --self-test` from somewhere else
# stops resolving once this script has moved. The self-test copies this file, so it needs this.
self=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
self=$self/$(basename -- "$0")
cd "$(dirname "$0")/.."

usage() {
  echo "Usage: scripts/build-deb.sh [--binary <path>] [version]"
  echo
  echo "  version          package version; defaults to the version in Cargo.toml"
  echo "  --binary <path>  the static binary to package; defaults to target/docker/apt-scout-plain"
  echo
  echo "Writes dist/apt-scout_<version>_amd64.deb from target/docker/apt-scout-plain, or from the file"
  echo "--binary names, which must be a regular executable file. The default comes from"
  echo "scripts/docker-test.sh --build-only; this script never builds a binary."
  echo "Builds with dpkg-deb on the host, or in an ubuntu:24.04 container when the host has none."
  echo
  echo "  --self-test  check the Cargo.toml readers and the --binary packaging run, then exit"
}

die() { echo "error: $*" >&2; exit 1; }

# One key out of one Cargo manifest section, and nothing else. A section-blind reader (a plain `sed`
# over the whole file) returns the first match anywhere, so a key that also exists in [package]
# silently wins over the one under [package.metadata.deb]. Each reader below names its section and
# is anchored to that section's header; keep them apart, because the version genuinely lives in
# [package] while the packaging fields live in [package.metadata.deb].
manifest=Cargo.toml
section_field() {
  awk -v section="[$1]" -v key="$2" '
    /^[ \t]*\[/ {
      header = $0
      gsub(/^[ \t]+|[ \t]+$/, "", header)
      inside = (header == section)
      next
    }
    !inside { next }
    {
      line = $0
      gsub(/^[ \t]+|[ \t]+$/, "", line)
      eq = index(line, "=")
      if (eq < 2) next
      name = substr(line, 1, eq - 1)
      gsub(/[ \t]+$/, "", name)
      if (name != key) next
      value = substr(line, eq + 1)
      gsub(/^[ \t]+|[ \t]+$/, "", value)
      if (value !~ /^"[^"]*"$/) exit
      gsub(/^"|"$/, "", value)
      print value
      exit
    }
  ' "$manifest"
}
# The crate's own fields: everything dpkg-deb does not need but the build reads.
package_field() { section_field package "$1"; }
# The fields dpkg-deb requires, declared one place: [package.metadata.deb] in Cargo.toml.
deb_field() { section_field package.metadata.deb "$1"; }
# A field dpkg-deb requires and the manifest must supply: its absence stops the build, it never
# falls back to a default here.
meta_required() {
  local value
  value=$(deb_field "$1")
  [ -n "$value" ] || die "cannot read [package.metadata.deb] $1 from Cargo.toml"
  printf '%s\n' "$value"
}

# The reader contract, checked against a manifest built to break it: a key in the wrong section is
# never the answer, whichever direction it comes from. Then --binary and the packaging run it feeds,
# built for real against a scratch tree and a fake binary. Run with --self-test.
self_test() {
  local dir bad=0
  dir=$(mktemp -d "${TMPDIR:-/tmp}/apt-scout-metadata.XXXXXX") || die "cannot create a scratch directory"
  manifest=$dir/Cargo.toml
  cat > "$manifest" <<'EOF'
[package]
name = "apt-scout"
version = "0.1.0"
description = "from the package table"
maintainer = "shadow in package <shadow@example.invalid>"

[package.metadata.deb]
maintainer = "real <real@example.invalid>"
section = "admin"
version = "9.9.9"
depends = "gpgv, ubuntu-keyring"
priority = "optional"
EOF
  check() {
    if [ "$2" = "$3" ]; then echo "PASS $1"; else echo "FAIL $1: expected '$2', got '$3'"; bad=1; fi
  }
  check "package_field version ignores the metadata.deb copy" 0.1.0 "$(package_field version)"
  check "deb_field version is the metadata.deb one" 9.9.9 "$(deb_field version)"
  check "deb_field maintainer ignores the [package] copy" "real <real@example.invalid>" "$(deb_field maintainer)"
  check "package_field maintainer is [package]'s own" "shadow in package <shadow@example.invalid>" "$(package_field maintainer)"
  check "package_field description is [package]'s own" "from the package table" "$(package_field description)"
  check "a key in neither section reads empty" "" "$(deb_field homepage)"

  # Then the packaging run itself, for real: a scratch copy of the tree (this script, the scratch
  # manifest, the two files a run installs) is packaged with --binary pointing at a fake binary, and
  # the archive's own usr/bin/apt-scout is compared with that file byte for byte. The other half is
  # the three refusals, each with its own exit status and its own message: a path that is not there,
  # a directory that is executable but is not a file — the one case only `-f` catches, because `-x`
  # passes on a directory — and a regular file without the execute bit.
  local repo=$dir/repo fake=$dir/fake-binary deb extract=$dir/extract out status
  deb=$repo/dist/apt-scout_9.9.9_amd64.deb
  mkdir -p "$repo/scripts"
  cp "$self" "$repo/scripts/build-deb.sh"
  chmod 0755 "$repo/scripts/build-deb.sh"
  cp "$manifest" "$repo/Cargo.toml"
  cp README.md LICENSE-MIT "$repo/"
  printf '#!/bin/sh\nexit 0\n' > "$fake"
  chmod 0755 "$fake"
  # Read a file back out of a built archive with whichever tool built it, the same two choices the
  # build has: dpkg-deb on the host, or the container. --user keeps the extracted tree removable by
  # the user that owns the scratch directory it sits in.
  deb_extract() {
    if command -v dpkg-deb >/dev/null 2>&1; then
      dpkg-deb -x "$1" "$2"
    else
      docker run --rm --platform linux/amd64 --user "$(id -u):$(id -g)" -v "$dir":/t \
        ubuntu:24.04 dpkg-deb -x "/t${1#"$dir"}" "/t${2#"$dir"}"
    fi
  }
  if out=$("$repo/scripts/build-deb.sh" --binary "$dir/not-there" 9.9.9 2>&1); then
    echo "FAIL --binary with a path that is not there exits 0"; bad=1
  else
    case $out in
      *"$dir/not-there"*) echo "PASS --binary with a path that is not there stops the build naming it" ;;
      *) echo "FAIL --binary with a path that is not there does not name it: $out"; bad=1 ;;
    esac
  fi
  # The case the `-f` guard exists for: a directory has the execute bit, so `-x` passes on it and a
  # build without the `-f` check reaches the staging `install` instead of stopping here.
  status=0
  out=$("$repo/scripts/build-deb.sh" --binary "$dir" 9.9.9 2>&1) || status=$?
  if [ "$status" -ne 1 ]; then
    echo "FAIL --binary with a directory exits $status, not 1"; bad=1
  else
    case $out in
      *"$dir is not a regular file"*) echo "PASS --binary with a directory says it is not a regular file" ;;
      *) echo "FAIL --binary with a directory does not say it is not a regular file: $out"; bad=1 ;;
    esac
  fi
  # The third truth: a regular file that is there and lacks the execute bit is neither missing nor a
  # directory, and says so instead of borrowing either message.
  printf '#!/bin/sh\nexit 0\n' > "$dir/not-executable"
  chmod 0644 "$dir/not-executable"
  status=0
  out=$("$repo/scripts/build-deb.sh" --binary "$dir/not-executable" 9.9.9 2>&1) || status=$?
  if [ "$status" -ne 1 ]; then
    echo "FAIL --binary with a regular file that is not executable exits $status, not 1"; bad=1
  else
    case $out in
      *"$dir/not-executable is not executable"*) echo "PASS --binary with a file that is not executable says so" ;;
      *) echo "FAIL --binary with a file that is not executable does not say so: $out"; bad=1 ;;
    esac
  fi
  if ! "$repo/scripts/build-deb.sh" --binary "$fake" 9.9.9 >/dev/null; then
    echo "FAIL --binary did not build a package from the given file"; bad=1
  elif [ ! -f "$deb" ]; then
    echo "FAIL --binary built no ${deb##*/}"; bad=1
  elif deb_extract "$deb" "$extract" && cmp -s "$fake" "$extract/usr/bin/apt-scout"; then
    echo "PASS --binary puts the given file's bytes at usr/bin/apt-scout"
  else
    echo "FAIL usr/bin/apt-scout is not the file --binary named"; bad=1
  fi

  rm -rf "$dir"
  [ "$bad" = 0 ] || die "the self-test failed: the lines marked FAIL above say which check"
}

# The default binary is the one scripts/docker-test.sh --build-only writes; --binary is how a caller
# with its own build directory (CI, whose cross build lands under target/<triple>) points the same
# builder at its own file.
binary=target/docker/apt-scout-plain
version=
while [ "$#" -gt 0 ]; do
  case $1 in
    -h|--help) usage; exit 0 ;;
    --self-test) self_test; exit 0 ;;
    --binary)
      [ "$#" -ge 2 ] || { echo "--binary needs a path; try --help" >&2; exit 2; }
      binary=$2
      shift ;;
    -*) echo "unknown option '$1'; try --help" >&2; exit 2 ;;
    *)
      [ -z "$version" ] || { echo "too many arguments; try --help" >&2; exit 2; }
      version=$1 ;;
  esac
  shift
done
if [ -z "$version" ]; then
  version=$(package_field version)
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
synopsis=$(package_field description)
[ -n "$synopsis" ] || die "cannot read the description from Cargo.toml"

# Three different truths get three different messages: a path that is not there, a path that is
# there but is not a regular file (a directory passes `-x`, so `-f` is the guard that catches it),
# and a regular file without the execute bit. Same exit code, no wrong message for a real path.
if [ ! -f "$binary" ]; then
  if [ -e "$binary" ]; then
    die "$binary is not a regular file; --binary names the file to package, and a directory is not one"
  fi
  die "$binary is missing; build it with scripts/docker-test.sh --build-only"
fi
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
