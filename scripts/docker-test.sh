#!/usr/bin/env bash
# Run apt-scout in real Ubuntu images (20.04 - 26.04), offline. Local only: CI does not run it.
# Usage: scripts/docker-test.sh [--build-only | --write-only] [ubuntu-version ...] (see --help)
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  echo "Usage: scripts/docker-test.sh [--build-only | --write-only] [ubuntu-version ...]"
  echo
  echo "  (no mode)           build both binaries, run the thirteen read-only cases, then the write cases"
  echo "  --build-only        build both binaries, print their digests, run no case"
  echo "  --write-only        build both binaries, then run only the write cases"
  echo "  ubuntu-version ...  limit the Ubuntu loops to these versions (default: all four)"
  echo
  echo "Every mode builds both binaries: the plain one is what the read-only cases run, the cfg one"
  echo "(target/docker/apt-scout-harness) is the only writer the write cases run."
}

# A mode flag never looks like a version: flags start with `-`, versions are `24.04` and friends,
# so a leading flag cannot select a version and a version cannot select a mode.
mode=default
versions=()
for arg in "$@"; do
  case $arg in
    --build-only) mode=build ;;
    --write-only) mode=write ;;
    --help|-h) usage; exit 0 ;;
    -*) echo "unknown option '$arg'; try --help" >&2; exit 2 ;;
    *) versions+=("$arg") ;;
  esac
done
# The loops below filter on "$*": the versions stay there and the mode flags stay out.
if [ "${#versions[@]}" -gt 0 ]; then set -- "${versions[@]}"; else set --; fi

# SPEC.md supports amd64 (and i386). Build and run amd64 everywhere; on Apple Silicon Docker emulates it.
target=x86_64-unknown-linux-musl
platform=linux/amd64

# Build a static binary with the same prebuilt cross musl image CI uses (it carries the musl C
# compiler that `ring` needs and the Rust toolchain both builds below run on).
# `target/docker-cargo` is a persistent CARGO_HOME, so the registry cache survives a run.
builder=ghcr.io/cross-rs/$target:main
docker pull --quiet --platform "$platform" "$builder" >/dev/null
builder_run() { docker run --rm --platform "$platform" -v "$PWD":/src -w /src \
  -e CARGO_TARGET_DIR=/src/target/docker -e CARGO_HOME=/src/target/docker-cargo "$builder" "$@"; }
# build plain|cfg -- `cargo clean -p apt-scout` drops only this crate's artifacts, never its deps,
# and never the two copies below (they live outside target/docker/$target). The clean is what makes
# a second invocation honest: `cargo rustc`'s trailing --cfg is not part of cargo's fingerprint, so
# without it run 2 would skip the plain build and copy the cfg artifact over the plain copy.
build() {
  case $1 in
    plain) cmd="cargo build --release --locked --target $target" ;;
    cfg)   cmd="cargo rustc --release --locked --target $target --bin apt-scout -- --cfg apt_scout_write_harness" ;;
  esac
  builder_run sh -c ". /src/target/docker-cargo/env 2>/dev/null || . \$HOME/.cargo/env
    cargo clean -p apt-scout >/dev/null && $cmd"
}
build plain; cp "target/docker/$target/release/apt-scout" target/docker/apt-scout-plain
build cfg;   cp "target/docker/$target/release/apt-scout" target/docker/apt-scout-harness
echo "PLAIN $(sha256sum target/docker/apt-scout-plain | cut -d' ' -f1)"
echo "HARNESS $(sha256sum target/docker/apt-scout-harness | cut -d' ' -f1)"
if [ "$mode" = build ]; then exit 0; fi
# The cfg build leaves its own artifact in the build directory, so the plain copy is what every
# read-only case below mounts: pointing `bin` at the build directory would silently run the harness
# in all thirteen of them.
bin=target/docker/apt-scout-plain

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

# The plain and cfg copies the build step above made. `wplain` is the same file `bin` names.
wplain=target/docker/apt-scout-plain      # the plain binary; `bin` is re-pointed at this copy
wharness=target/docker/apt-scout-harness  # the cfg binary
WFIXTURE=apt-scout-fixture:local          # the committed read-only fixture, made by wfixture

# The one /etc/apt layout the write cases start from: written by the case itself, as root, inside
# its own container, so no case depends on what the image happens to ship. It guarantees the
# replace target is sources.list.d/ubuntu.sources (no sources.list, exactly one *.sources).
FIX_DEB822='rm -f /etc/apt/sources.list; rm -rf /etc/apt/sources.list.d; mkdir -p /etc/apt/sources.list.d; printf "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble noble-updates\nComponents: main universe\n" > /etc/apt/sources.list.d/ubuntu.sources; chmod 0644 /etc/apt/sources.list.d/ubuntu.sources'

# The only way a write case runs: the harness binary, read-only, offline, and no other mount. A
# write case that names the network mode itself is the one thing this file must never grow.
wrun() { docker run --rm --network none --platform "$platform" -v "$PWD/$wharness":/apt-scout:ro "$@"; }

# The read-only-rootfs cases (W10, W11): that rootfs is their control, so these flags live here and
# nowhere else.
wro() { wrun --read-only --tmpfs /tmp "$@"; }

# The read-only cases' pin: one throwaway container writes FIX_DEB822 and is then frozen into
# $WFIXTURE, because no setup at all can run against a read-only rootfs. Offline, mounts nothing.
wfixture() {
  docker rm -f apt-scout-fixture-build >/dev/null 2>&1 || true
  docker run --name apt-scout-fixture-build --network none --platform "$platform" ubuntu:24.04 \
    sh -c "$FIX_DEB822" >/dev/null
  docker commit apt-scout-fixture-build "$WFIXTURE" >/dev/null
  docker rm apt-scout-fixture-build >/dev/null
}

# The container-side preamble every write case reuses. `snapshot` covers type, mode, owner, every
# path and every byte under /etc/apt, so a temp file, a created file, a mode change and a content
# change all show up. `digests` covers the files the case names in WCASE_DIGESTS; the list is handed
# to the container by `wcase`, so one case's list can never be the one another case measured.
WCASE_PRE="snapshot() { find /etc/apt -mindepth 1 -printf '%y %m %u:%g %p\n' | sort > \"\$1\"; find /etc/apt -type f | sort | xargs -r sha256sum >> \"\$1\"; }; digests() { sha256sum \$WCASE_DIGESTS 2>/dev/null || echo \"missing \$WCASE_DIGESTS\"; }"

# wcase <image> <setup> <command>
#   $WCASE_DIGESTS is a host-side, space-separated list of files (globs allowed) that must be
#   byte-identical before and after the run; `digests` is only called when the list is non-empty,
#   so an unset list can never make `sha256sum` read standard input.
#   Prints, in order: rc=<n>; same=yes|no (+ diff); digests=unchanged|changed|none (+ the after
#   list, always, so a case can name the digest it expects); backups=yes|no; then the command's own
#   output. A glob that matches nothing prints `missing <glob>`, so an empty directory that gains a
#   file shows up as `digests=changed`.
#   The setup is never empty: every write case pins its /etc/apt layout.
wcase() {
  local files=${WCASE_DIGESTS:-}
  WCASE_DIGESTS=  # one case's list never reaches the next one
  wrun "$1" sh -c "WCASE_DIGESTS='$files'
$WCASE_PRE
$2
snapshot /tmp/before
if [ -n \"\$WCASE_DIGESTS\" ]; then digests > /tmp/d-before; fi
rc=0; $3 > /tmp/out 2>&1 || rc=\$?
echo rc=\$rc
snapshot /tmp/after
if cmp -s /tmp/before /tmp/after; then echo same=yes; else echo same=no; diff -u /tmp/before /tmp/after; fi
if [ -n \"\$WCASE_DIGESTS\" ]; then digests > /tmp/d-after; if cmp -s /tmp/d-before /tmp/d-after; then echo digests=unchanged; else echo digests=changed; fi; cat /tmp/d-after; else echo digests=none; fi
if [ -e /var/backups/apt-scout ]; then echo backups=yes; else echo backups=no; fi
cat /tmp/out"
}

# --write-only skips the thirteen read-only cases below, which stay byte-for-byte as they were:
# the write cases run in both modes, so a write case's result never depends on which mode started
# it.
if [ "$mode" != write ]; then
# 1. Offline: the release codename is detected on every supported Ubuntu, and geolocation fails
#    gracefully before the program asks for an explicit country (exit 2).
matrix="20.04 focal
22.04 jammy
24.04 noble
26.04 resolute"
ran=0
while read -r v codename; do
  if [ $# -gt 0 ]; then case " $* " in *" $v "*) ;; *) continue ;; esac; fi
  ran=$((ran + 1))
  rc=0; got=$(run none "ubuntu:$v" /apt-scout 2>&1) || rc=$?
  if [ "$rc" -eq 2 ] && [[ "$got" == *"cannot tell which country"* ]]; then pass "ubuntu:$v offline -> graceful geolocation fallback (exit 2)"; else fail "ubuntu:$v offline -> exit $rc: $got"; fi
done <<EOF
$matrix
EOF
if [ "$ran" -eq 0 ]; then echo "FAIL no Ubuntu version matched: $*" >&2; exit 1; fi

# 2. Offline: Mint-style os-release is accepted (ID_LIKE has ubuntu).
rc=0; got=$(run none ubuntu:24.04 sh -c \
  'printf "ID=linuxmint\nID_LIKE=\"ubuntu debian\"\nVERSION_CODENAME=wilma\nUBUNTU_CODENAME=noble\n" > /etc/os-release; /apt-scout' 2>&1) || rc=$?
if [ "$rc" -eq 2 ] && [[ "$got" == *"cannot tell which country"* ]]; then pass "mint os-release accepted"; else fail "mint -> exit $rc: $got"; fi

# 3. Offline: --no-geo requires explicit country and does not access network.
rc=0; got=$(run none ubuntu:24.04 /apt-scout --no-geo 2>&1) || rc=$?
if [ "$rc" = 2 ] && [[ "$got" == *"--no-geo needs --country"* ]]; then pass "--no-geo without country -> clear usage error"; else fail "--no-geo without country -> exit $rc: $got"; fi

# 4. Offline: missing codename names the source and does not suggest an unsupported flag.
rc=0; got=$(run none ubuntu:24.04 sh -c 'printf "ID=ubuntu\n" > /etc/os-release; /apt-scout' 2>&1) || rc=$?
if [ "$rc" -eq 1 ] && [[ "$got" == *"/etc/os-release contains neither"* ]] && [[ "$got" != *"--codename"* ]]; then pass "missing codename -> actionable error, exit 1"; else fail "missing codename -> exit $rc: $got"; fi

# 5. Offline: Debian is rejected with exit 4.
rc=0; run none debian:12 /apt-scout >/dev/null 2>&1 || rc=$?
[ "$rc" = 4 ] && pass "debian -> exit 4" || fail "debian -> exit $rc"

# 6. Offline: a syntactically valid but unsupported country is rejected before geolocation.
rc=0; got=$(run none ubuntu:24.04 /apt-scout --country ZZ 2>&1) || rc=$?
if [ "$rc" = 2 ] && [[ "$got" == *"unknown country code 'ZZ'"* ]] && [[ "$got" != *"note: geolocation"* ]] && [[ "$got" != *"cannot reach"* ]]; then pass "unknown country -> rejected before network"; else fail "unknown country -> exit $rc: $got"; fi

# 7. Offline: unsupported architecture detected from dpkg is rejected before geolocation/network.
rc=0; got=$(run none ubuntu:24.04 sh -c 'mkdir -p /tmp/fakebin; printf "#!/bin/sh\\necho arm64\\n" > /tmp/fakebin/dpkg; chmod +x /tmp/fakebin/dpkg; PATH=/tmp/fakebin:/usr/bin:/bin /apt-scout --country LT' 2>&1) || rc=$?
if [ "$rc" = 4 ] && [[ "$got" == *"architecture 'arm64'"* ]] && [[ "$got" != *"note: geolocation"* ]] && [[ "$got" != *"cannot reach"* ]]; then pass "unsupported architecture -> rejected before network"; else fail "unsupported architecture -> exit $rc: $got"; fi

# 8. Offline: bad arguments are usage errors (exit 2) that say what to do.
rc=0; got=$(run none ubuntu:24.04 /apt-scout --country LTU 2>&1) || rc=$?
[ "$rc" = 2 ] && [[ "$got" == *"not a two-letter country code"* ]] && pass "bad --country -> exit 2" || fail "bad --country -> exit $rc: $got"
rc=0; got=$(run none ubuntu:24.04 /apt-scout --bogus 2>&1) || rc=$?
[ "$rc" = 2 ] && [[ "$got" == *"unknown option '--bogus'"* ]] && pass "unknown option -> exit 2" || fail "unknown option -> exit $rc: $got"

# 9. Offline: a network failure names the URL and the cause and exits 1 (no panic, no hang).
rc=0; got=$(run none ubuntu:24.04 /apt-scout --country LT 2>&1) || rc=$?
if [ "$rc" = 1 ] && [[ "$got" == *"no mirror list is available"* ]] && [[ "$got" == *"launchpad.net"* ]] && [[ "$got" == *"mirrors.ubuntu.com/LT.txt"* ]]; then pass "no network -> clear error, exit 1"; else fail "no network -> exit $rc: $got"; fi

# 10. Live: with only third-party apt sources, inventory must stop before reference/mirror probing.
rc=0; got=$(run bridge ubuntu:24.04 sh -c 'rm -f /etc/apt/sources.list; rm -rf /etc/apt/sources.list.d; mkdir -p /etc/apt/sources.list.d; printf "deb https://packages.example.org/ubuntu noble main\\n" > /etc/apt/sources.list.d/third-party.list; /apt-scout --no-geo --country LT' 2>&1) || rc=$?
if [ "$rc" = 0 ] && [[ "$got" == *"APT source inventory (read-only)"* ]] && [[ "$got" == *"not an official Ubuntu archive"* ]] && [[ "$got" == *"No official Ubuntu apt source URI can be changed"* ]] && [[ "$got" != *"baseline"* ]] && [[ "$got" != *"ranked"* ]]; then pass "third-party-only apt sources -> stop before probes"; else fail "third-party-only sources -> exit $rc: $(echo "$got" | head -8)"; fi

# 11. Live: missing gpgv keeps ranking read-only usable but marks signatures unavailable.
rc=0; got=$(run bridge ubuntu:24.04 sh -c 'mv /usr/bin/gpgv /usr/bin/gpgv.disabled 2>/dev/null || true; /apt-scout --no-geo --country LT --top 2' 2>&1) || rc=$?
if [ "$rc" = 0 ] && [[ "$got" == *"unavailable ("* ]] && [[ "$got" == *" 1  http"* ]]; then pass "missing gpgv -> unavailable signatures, ranking still works"; else fail "missing gpgv -> exit $rc: $(echo "$got" | head -8)"; fi

# 12. Live (needs internet): real Launchpad + real mirrors, every supported Ubuntu release.
#    Checks shape, not specific mirrors: a ranked table with at least one fresh mirror.
for v in 20.04 22.04 24.04 26.04; do
  if [ $# -gt 0 ]; then case " $* " in *" $v "*) ;; *) continue ;; esac; fi
  rc=0; got=$(run bridge "ubuntu:$v" /apt-scout --country LT --top 3 2>&1) || rc=$?
  if [ "$rc" = 0 ] && [[ "$got" == *"country Lithuania"* ]] && [[ "$got" == *"APT source inventory (read-only)"* ]] && [[ "$got" == *"source file: /etc/apt/"* ]] && [[ "$got" == *" 1  http"* ]]; then pass "ubuntu:$v live LT -> source inventory and ranked table"; else fail "ubuntu:$v live LT -> exit $rc: $(echo "$got" | head -6)"; fi
done

# 13. Live: a real verified row. The base images have no ubuntu-keyring, so install the two
#     dependencies the .deb declares before asserting the positive path. Exit 9 means that install
#     failed, not that apt-scout is broken.
rc=0; got=$(run bridge ubuntu:24.04 sh -c 'apt-get update -qq >/dev/null 2>&1; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gpgv ubuntu-keyring >/dev/null 2>&1 || { echo DEPENDENCY_INSTALL_FAILED; exit 9; }; /apt-scout --no-geo --country LT --top 3' 2>&1) || rc=$?
if [ "$rc" = 0 ] && [[ "$got" == *"Reference InRelease signature: verified"* ]] && printf '%s\n' "$got" | grep -qE 'verified +[0-9]+m'; then
  pass "live LT -> reference and at least one mirror verified"
else
  fail "live LT verified row -> exit $rc: $(echo "$got" | head -12)"
fi
fi # end of the read-only cases: skipped only by --write-only

# --- Write cases ------------------------------------------------------------------------------
# W1 and W2 prove the plain binary cannot reach the harness, W3 the container guard, W4 the two
# fixture-derived dates W17 depends on. The cases that write come after these.

# W1 (S16) Offline, host-side: the shipped binary carries no harness code, the cfg build does.
#   The marker is a harness-only message, never the option's spelling: a comparison against a
#   15-byte constant is folded into immediate compares by an optimised build, so not even the cfg
#   binary contains the literal `__write-harness` (checked against this crate's own release build).
harness_mark='unknown write-harness case'
rc=0; plain_hits=$(rg -a -c -F "$harness_mark" "$wplain" 2>/dev/null) || rc=$?
harn_rc=0; harness_hits=$(rg -a -c -F "$harness_mark" "$wharness" 2>/dev/null) || harn_rc=$?
if [ "$rc" -eq 1 ] && [ -z "$plain_hits" ] && [ "$harn_rc" -eq 0 ] && [ "${harness_hits:-0}" -ge 1 ]; then
  pass "W1 plain build has no write harness code, cfg build has ($harness_hits hits)"
else
  fail "W1 plain build rc=$rc hits='$plain_hits', cfg build rc=$harn_rc hits='$harness_hits'"
fi

# W2 (S16) Offline, plain binary: the harness option is an ordinary unknown argument, so the
# shipped binary cannot be driven into a write case by name.
rc=0; got=$(run none ubuntu:24.04 sh -c '/apt-scout --help; echo HELP_END; /apt-scout __write-harness apply; echo rc=$?' 2>&1) || rc=$?
help_out=${got%%HELP_END*}; plain_out=${got#*HELP_END}
if [ "$rc" -eq 0 ] && [[ "$help_out" != *"write-harness"* ]] && [[ "$plain_out" == *"unknown option '__write-harness'"* ]] && [[ "$plain_out" == *"rc=2"* ]]; then
  pass "W2 plain binary -> no harness in --help, __write-harness is an unknown option (exit 2)"
else
  fail "W2 plain binary -> exit $rc: help='$(echo "$help_out" | head -3)' run='$(echo "$plain_out" | head -3)'"
fi

# W3 (S16) Offline: outside a container the guard refuses before anything is opened, so /etc/apt
# is untouched as well as unwritten (the case can be run anywhere without a pin).
rc=0; got=$(wrun ubuntu:24.04 sh -c "$WCASE_PRE
rm -f /.dockerenv /run/.containerenv
snapshot /tmp/before
rc=0; /apt-scout __write-harness apply 2>&1 || rc=\$?
echo rc=\$rc
snapshot /tmp/after
cmp -s /tmp/before /tmp/after && echo same=yes || echo same=no" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] && [[ "$got" == *"refusing to run a write case outside a container"* ]] && [[ "$got" != *"OK backup="* ]] && [[ "$got" == *"same=yes"* ]]; then
  pass "W3 outside a container -> refusing guard, exit 1, /etc/apt untouched"
else
  fail "W3 outside a container -> exit $rc: $(echo "$got" | head -6)"
fi

# W4 Offline: the harness's own dates are the fixture's, and its stale constant is past the limit.
rc=0; got=$(wrun ubuntu:24.04 /apt-scout __write-harness selftest 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == HARNESS* ]] && [[ "$got" == *"stale=yes"* ]]; then
  pass "W4 selftest -> $(echo "$got" | head -1)"
else
  fail "W4 selftest -> exit $rc: $(echo "$got" | head -3)"
fi

exit $status
