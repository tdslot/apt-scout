#!/usr/bin/env bash
# Run apt-scout in real Ubuntu images (20.04 - 26.04), offline. Local only: CI does not run it.
# Usage: scripts/docker-test.sh [ubuntu-version ...]
set -euo pipefail
cd "$(dirname "$0")/.."

# SPEC.md supports amd64 (and i386). Build and run amd64 everywhere; on Apple Silicon Docker emulates it.
target=x86_64-unknown-linux-musl
platform=linux/amd64

# Build a static binary with the same prebuilt cross musl image CI uses (it carries the musl C
# compiler that `ring` needs). Rust itself is installed by the image's rustup on first use.
builder=ghcr.io/cross-rs/$target:main
docker pull --quiet --platform "$platform" "$builder" >/dev/null
docker run --rm --platform "$platform" -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/docker \
  -e CARGO_HOME=/src/target/docker-cargo "$builder" \
  sh -c "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable -t $target >/dev/null 2>&1 && . /src/target/docker-cargo/env 2>/dev/null || . \$HOME/.cargo/env; cargo build --release --locked --target $target"
bin=target/docker/$target/release/apt-scout

# Pull every image first so `docker run` prints only the program's output (a fresh runner would
# otherwise add "Unable to find image..." lines to the output being compared).
for image in ubuntu:20.04 ubuntu:22.04 ubuntu:24.04 ubuntu:26.04 debian:12; do
  docker pull --quiet --platform "$platform" "$image" >/dev/null
done

# Run the binary in an image; the platform is explicit so Docker prints no mismatch warning.
# `net` is "none" for offline cases (they must not need the network) or "bridge" for live ones.
run() { local net=$1; shift; docker run --rm --network "$net" --platform "$platform" -v "$PWD/$bin":/apt-scout:ro "$@"; }
status=0
pass() { echo "PASS $1"; }
fail() { echo "FAIL $1"; status=1; }

# 1. Offline: the release codename is detected on every supported Ubuntu, and the program stops
#    with the "no country" usage error (exit 2) before any network call.
matrix="20.04 focal
22.04 jammy
24.04 noble
26.04 resolute"
ran=0
while read -r v codename; do
  if [ $# -gt 0 ]; then case " $* " in *" $v "*) ;; *) continue ;; esac; fi
  ran=$((ran + 1))
  rc=0; got=$(run none "ubuntu:$v" /apt-scout 2>&1) || rc=$?
  if [ "$rc" -eq 2 ] && [[ "$got" == *"no country given"* ]]; then pass "ubuntu:$v offline -> reaches country check (exit 2)"; else fail "ubuntu:$v offline -> exit $rc: $got"; fi
done <<EOF
$matrix
EOF
if [ "$ran" -eq 0 ]; then echo "FAIL no Ubuntu version matched: $*" >&2; exit 1; fi

# 2. Offline: Mint-style os-release is accepted (ID_LIKE has ubuntu).
rc=0; got=$(run none ubuntu:24.04 sh -c \
  'printf "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_CODENAME=wilma\nUBUNTU_CODENAME=noble\n" > /etc/os-release; /apt-scout' 2>&1) || rc=$?
if [ "$rc" -eq 2 ] && [[ "$got" == *"no country given"* ]]; then pass "mint os-release accepted"; else fail "mint -> exit $rc: $got"; fi

# 3. Offline: missing codename names the source and does not suggest an unsupported flag.
rc=0; got=$(run none ubuntu:24.04 sh -c 'printf "ID=ubuntu\n" > /etc/os-release; /apt-scout' 2>&1) || rc=$?
if [ "$rc" -eq 1 ] && [[ "$got" == *"/etc/os-release contains neither"* ]] && [[ "$got" != *"--codename"* ]]; then pass "missing codename -> actionable error, exit 1"; else fail "missing codename -> exit $rc: $got"; fi

# 4. Offline: Debian is rejected with exit 4.
rc=0; run none debian:12 /apt-scout >/dev/null 2>&1 || rc=$?
[ "$rc" = 4 ] && pass "debian -> exit 4" || fail "debian -> exit $rc"

# 5. Offline: bad arguments are usage errors (exit 2) that say what to do.
rc=0; got=$(run none ubuntu:24.04 /apt-scout --country LTU 2>&1) || rc=$?
[ "$rc" = 2 ] && [[ "$got" == *"not a two-letter country code"* ]] && pass "bad --country -> exit 2" || fail "bad --country -> exit $rc: $got"
rc=0; got=$(run none ubuntu:24.04 /apt-scout --bogus 2>&1) || rc=$?
[ "$rc" = 2 ] && [[ "$got" == *"unknown option '--bogus'"* ]] && pass "unknown option -> exit 2" || fail "unknown option -> exit $rc: $got"

# 6. Offline: a network failure names the URL and the cause and exits 1 (no panic, no hang).
rc=0; got=$(run none ubuntu:24.04 /apt-scout --country LT 2>&1) || rc=$?
if [ "$rc" = 1 ] && [[ "$got" == *"cannot load the mirror list"* ]] && [[ "$got" == *"launchpad.net"* ]]; then pass "no network -> clear error, exit 1"; else fail "no network -> exit $rc: $got"; fi

# 7. Live (needs internet): real Launchpad + real mirrors, every supported Ubuntu release.
#    Checks shape, not specific mirrors: a ranked table with at least one fresh mirror.
for v in 20.04 22.04 24.04 26.04; do
  if [ $# -gt 0 ]; then case " $* " in *" $v "*) ;; *) continue ;; esac; fi
  rc=0; got=$(run bridge "ubuntu:$v" /apt-scout --country LT --top 3 2>&1) || rc=$?
  if [ "$rc" = 0 ] && [[ "$got" == *"country Lithuania"* ]] && [[ "$got" == *" 1  http"* ]]; then pass "ubuntu:$v live LT -> ranked table"; else fail "ubuntu:$v live LT -> exit $rc: $(echo "$got" | head -4)"; fi
done
exit $status
