#!/usr/bin/env bash
# Run apt-scout in real Ubuntu images (20.04 - 26.04), offline. Same script locally and in CI.
# Usage: scripts/docker-test.sh [ubuntu-version ...]
set -euo pipefail
cd "$(dirname "$0")/.."

# SPEC.md supports amd64 (and i386). Build and run amd64 everywhere; on Apple Silicon Docker emulates it.
target=x86_64-unknown-linux-musl
platform=linux/amd64

# Build a static binary inside a Linux container.
docker run --rm --platform "$platform" -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/docker rust:1 \
  sh -c "rustup target add $target >/dev/null 2>&1 && cargo build --release --locked --target $target"
bin=target/docker/$target/release/apt-scout

# Pull every image first so `docker run` prints only the program's output (a fresh runner would
# otherwise add "Unable to find image..." lines to the output being compared).
for image in ubuntu:20.04 ubuntu:22.04 ubuntu:24.04 ubuntu:26.04 debian:12; do
  docker pull --quiet --platform "$platform" "$image" >/dev/null
done

# Run the binary in an image; the platform is explicit so Docker prints no mismatch warning.
run() { docker run --rm --platform "$platform" -v "$PWD/$bin":/apt-scout:ro "$@"; }

# "<version> <expected id> <expected codename>" (no associative arrays: macOS ships bash 3.2)
matrix="20.04 ubuntu focal
22.04 ubuntu jammy
24.04 ubuntu noble
26.04 ubuntu resolute"
status=0
ran=0
while read -r v id codename; do
  if [ $# -gt 0 ] && ! printf '%s\n' "$@" | grep -qx "$v"; then continue; fi
  ran=$((ran + 1))
  got=$(run "ubuntu:$v" /apt-scout 2>&1) || true
  if [ "$got" = "$id $codename" ]; then echo "PASS ubuntu:$v -> $got"; else echo "FAIL ubuntu:$v -> '$got' (want '$id $codename')"; status=1; fi
done <<EOF
$matrix
EOF
if [ "$ran" -eq 0 ]; then echo "FAIL no Ubuntu version matched: $*" >&2; exit 1; fi

# Mint-style os-release: VERSION_CODENAME differs from UBUNTU_CODENAME
got=$(run ubuntu:24.04 sh -c \
  'printf "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_CODENAME=wilma\nUBUNTU_CODENAME=noble\n" > /etc/os-release; /apt-scout' 2>&1) || true
[ "$got" = "linuxmint noble" ] && echo "PASS mint -> $got" || { echo "FAIL mint -> '$got'"; status=1; }

# Debian must be rejected with exit code 4
run debian:12 /apt-scout >/dev/null 2>&1 && rc=0 || rc=$?
[ "$rc" = 4 ] && echo "PASS debian -> exit 4" || { echo "FAIL debian -> exit $rc"; status=1; }
exit $status
