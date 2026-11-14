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
  -e CARGO_TARGET_DIR=/src/target/docker \
  -e CARGO_HOME=/src/target/docker-cargo -e RUSTUP_HOME=/src/target/rustup "$builder" "$@"; }
# Both builds run in their own container, so the toolchain the musl target needs lives in the two
# persistent directories above: installed once per invocation, reused by the second build, and
# skipped by rustup itself when it is already there.
builder_prepare() {
  builder_run sh -c "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal \
    --default-toolchain stable -t $target >/dev/null 2>&1"
}
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
builder_prepare
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

# The legacy pin, for the case whose target is sources.list rather than an entry of sources.list.d:
# no sources.list.d at all, one single-line sources.list.
FIX_LEGACY='rm -rf /etc/apt/sources.list.d; rm -f /etc/apt/sources.list; printf "deb http://archive.ubuntu.com/ubuntu jammy main universe\n" > /etc/apt/sources.list; chmod 0644 /etc/apt/sources.list'

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

# --write-only skips the thirteen read-only cases below, which read and never write: the write
# cases run in both modes, so a write case's result never depends on which mode started it.
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
#    The binary reads `/usr/bin/dpkg`, not `dpkg` through `PATH` (SPEC.md Privilege boundary: a root
#    run's commands have fixed paths), so the fake replaces that file instead of shadowing the name.
rc=0; got=$(run none ubuntu:24.04 sh -c 'printf "#!/bin/sh\\necho arm64\\n" > /usr/bin/dpkg; chmod +x /usr/bin/dpkg; /apt-scout --country LT' 2>&1) || rc=$?
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
#     `--list` is the read-only mode whatever the uid (SPEC.md CLI table), so this case stays about
#     the inventory and never asks to write.
rc=0; got=$(run bridge ubuntu:24.04 sh -c 'rm -f /etc/apt/sources.list; rm -rf /etc/apt/sources.list.d; mkdir -p /etc/apt/sources.list.d; printf "deb https://packages.example.org/ubuntu noble main\\n" > /etc/apt/sources.list.d/third-party.list; /apt-scout --list --no-geo --country LT' 2>&1) || rc=$?
#    The inventory line the case reads is now the `Sources` heading plus the skip line naming the
#    host and the reason. `Ranked` and `reference:` are what a run that went on to probe would print,
#    so their absence is still what proves the early stop.
if [ "$rc" = 0 ] && printf '%s\n' "$got" | grep -q '^Sources ' && [[ "$got" == *"packages.example.org"* ]] && [[ "$got" == *"is not an official Ubuntu archive mirror"* ]] && [[ "$got" == *"No official Ubuntu apt source URI can be changed"* ]] && [[ "$got" != *"reference:"* ]] && [[ "$got" != *"Ranked"* ]]; then pass "third-party-only apt sources -> stop before probes"; else fail "third-party-only sources -> exit $rc: $(echo "$got" | head -8)"; fi

# The `notes` column is the table's last one and is printed only when a shown row has a fact for
# it, so a ranked row ends either at its `sig` cell or at up to two note tokens: `sec:<n>m` for a
# -security lag, `redir:<host>` for a redirect, either alone or both. Anchoring the tail to that
# grammar is what keeps `  ok$` a claim about the `sig` cell: any other tail misses, and a row whose
# signature is not `ok` misses too.
notes_tail='(  (sec:[0-9]+m|redir:[^ ]+|sec:[0-9]+m redir:[^ ]+))?$'

# 11. Live: missing gpgv keeps ranking read-only usable but marks signatures unavailable.
#     The case is about the ranking, and a root run without `--list` would try to write, so `--list`
#     keeps it on the read-only path it was written for.
rc=0; got=$(run bridge ubuntu:24.04 sh -c 'mv /usr/bin/gpgv /usr/bin/gpgv.disabled 2>/dev/null || true; /apt-scout --list --no-geo --country LT --top 2' 2>&1) || rc=$?
#    The `sig` cell reads `unavailable` for every row, and `--no-geo` is the run that has no AS data,
#    so the one note under the table is the run's own reason rather than a column of `unknown`. A row
#    may end at a `notes` cell (a live mirror can lag on -security or redirect); the anchor admits
#    only that grammar.
if [ "$rc" = 0 ] && printf '%s\n' "$got" | grep -qE "^ +[0-9]+ +https?://.*  unavailable${notes_tail}" && [[ "$got" == *"note: AS data unavailable (--no-geo skips every AS lookup)"* ]]; then pass "missing gpgv -> unavailable signatures, AS note says why, ranking still works"; else fail "missing gpgv -> exit $rc: $(echo "$got" | head -8)"; fi

# 12. Live (needs internet): real Launchpad + real mirrors, every supported Ubuntu release.
#    Checks shape, not specific mirrors: a ranked table with at least one fresh mirror. `--list` is
#    the read-only mode, so a root container reports the table and the planned change without writing.
for v in 20.04 22.04 24.04 26.04; do
  if [ $# -gt 0 ]; then case " $* " in *" $v "*) ;; *) continue ;; esac; fi
  rc=0; got=$(run bridge "ubuntu:$v" /apt-scout --list --country LT --top 3 2>&1) || rc=$?
  # The Sources heading names the apt root the relative paths below it hang from, and the rewrite
  # line is the inventory proving it read a real file; ` 1  http` is the first ranked row.
  if [ "$rc" = 0 ] && [[ "$got" == *"country: Lithuania (--country)"* ]] && printf '%s\n' "$got" | grep -q '^Sources .*/etc/apt$' && [[ "$got" == *"rewrite  line "* ]] && [[ "$got" == *" 1  http"* ]]; then pass "ubuntu:$v live LT -> source inventory and ranked table"; else fail "ubuntu:$v live LT -> exit $rc: $(echo "$got" | head -6)"; fi
done

# 13. Live: a real verified row. The base images have no ubuntu-keyring, so install the two
#     dependencies the .deb declares before asserting the positive path. Exit 9 means that install
#     failed, not that apt-scout is broken. `--list` keeps this about the verified ranking: a write
#     case would need the harness, not this binary.
rc=0; got=$(run bridge ubuntu:24.04 sh -c 'apt-get update -qq >/dev/null 2>&1; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gpgv ubuntu-keyring >/dev/null 2>&1 || { echo DEPENDENCY_INSTALL_FAILED; exit 9; }; /apt-scout --list --no-geo --country LT --top 3' 2>&1) || rc=$?
#    The positive path is two lines of the new report, each anchored so the other cannot stand in
#    for it: the context line says the freshness reference verified, and at least one ranked row's
#    `sig` cell reads `ok`, the only cell value a verified `-updates` InRelease produces. A run with
#    no keyring prints `unverified (...)` in that cell and `(unverified (...))` on the context line,
#    so both patterns miss; an empty or unrelated report matches neither either, because both are
#    anchored to the start and the end of the line they read. The row's end is its `sig` cell or a
#    `notes` cell after it, and only the note grammar above may stand there.
if [ "$rc" = 0 ] && printf '%s\n' "$got" | grep -qE "^ +[0-9]+ +https?://.*  ok${notes_tail}" \
  && printf '%s\n' "$got" | grep -qE '^server AS.*reference: archive\.ubuntu\.com \(verified\)$'; then
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
# shipped binary cannot be driven into a write case by name. The help's own shape is asserted here
# too: the sections a user reads, the `--verbose` flag, and the rule that only `ok` authorizes a
# write (it used to repeat under every ranked table).
rc=0; got=$(run none ubuntu:24.04 sh -c '/apt-scout --help; echo HELP_END; /apt-scout __write-harness apply; echo rc=$?' 2>&1) || rc=$?
help_out=${got%%HELP_END*}; plain_out=${got#*HELP_END}
if [ "$rc" -eq 0 ] && [[ "$help_out" != *"write-harness"* ]] && [[ "$help_out" == *"USAGE"* ]] && [[ "$help_out" == *"EXIT CODES"* ]] && [[ "$help_out" == *"--verbose"* ]] && [[ "$help_out" == *"only ok may authorize a write"* ]] && [[ "$plain_out" == *"unknown option '__write-harness'"* ]] && [[ "$plain_out" == *"rc=2"* ]]; then
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

# W5 (S1) Offline: the non-root half of S1. `setpriv` drops to uid 1000 inside the root container,
#   because the pin needs root and uid 1000 cannot write /etc/apt. A missing `setpriv` fails the
#   case loudly: skipping the only non-root case here would leave S1 half-unproven.
rc=0; got=$(wcase ubuntu:24.04 \
  "$FIX_DEB822; command -v setpriv >/dev/null || { echo WPIN setpriv missing; exit 1; }" \
  "setpriv --reuid=1000 --regid=1000 --clear-groups /apt-scout __write-harness apply" 2>&1) || rc=$?
if [[ "$got" == *"WPIN setpriv missing"* ]]; then
  fail "W5 ubuntu:24.04 ships no setpriv, so the non-root half of S1 could not run"
elif [ "$rc" -eq 0 ] && [[ "$got" == *"rc=3"* ]] && [[ "$got" == *"sudo apt-scout"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W5 non-root run -> sudo apt-scout, exit 3, /etc/apt identical"
else
  fail "W5 non-root run -> exit $rc: $(echo "$got" | head -6)"
fi

# The digest of the canary every symlink case writes: `printf 'canary-v1\n'`, so the assertion
# compares bytes that exist before the link does, never an ENOENT that would fail the setup.
canary=3a0995af8316bca3b555be990b0f759ed9539f0211d3095e11d939252003cb82

# W6 (S5) Offline: the target itself is a symlink out of the tree. `read_state`'s `lstat` sees the
#   link, so the refusal names the target and the reason instead of following it to the canary.
rc=0; got=$(WCASE_DIGESTS=/tmp/canary wcase ubuntu:24.04 \
  "$FIX_DEB822; printf 'canary-v1\n' > /tmp/canary; rm -f /etc/apt/sources.list.d/ubuntu.sources; ln -s /tmp/canary /etc/apt/sources.list.d/ubuntu.sources" \
  "/apt-scout __write-harness apply" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Target: refusing /etc/apt/sources.list.d/ubuntu.sources: it is a symlink"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"$canary  /tmp/canary"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W6 symlinked target -> ERR Target, exit 1, canary and /etc/apt identical"
else
  fail "W6 symlinked target -> exit $rc: $(echo "$got" | head -6)"
fi

# W7 (S5) Offline: the same refusal on the legacy single-file layout, where the target is
#   sources.list rather than an entry of sources.list.d.
rc=0; got=$(WCASE_DIGESTS=/tmp/canary wcase ubuntu:22.04 \
  "$FIX_LEGACY; printf 'canary-v1\n' > /tmp/canary; rm -f /etc/apt/sources.list; ln -s /tmp/canary /etc/apt/sources.list" \
  "/apt-scout __write-harness apply" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Target: refusing /etc/apt/sources.list: it is a symlink"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"$canary  /tmp/canary"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W7 symlinked sources.list -> ERR Target, exit 1, canary and /etc/apt identical"
else
  fail "W7 symlinked sources.list -> exit $rc: $(echo "$got" | head -6)"
fi

# W8 (S5) Offline: sources.list.d itself is a symlink to a directory outside the tree, so the
#   planned target would resolve through it. `check_parent` refuses the directory, not the file,
#   and the one pinned file beside the link keeps its digest.
evil=6eff4af09971bfade12236da91531b406f28c2b1ad1a487fbbddd1cbb634d3d7  # digest of the evil.sources bytes the setup writes
rc=0; got=$(WCASE_DIGESTS='/tmp/attacker/*' wcase ubuntu:24.04 \
  "$FIX_DEB822; rm -rf /etc/apt/sources.list.d; mkdir -p /tmp/attacker; printf 'deb http://archive.ubuntu.com/ubuntu noble main\n' > /tmp/attacker/evil.sources; ln -s /tmp/attacker /etc/apt/sources.list.d" \
  "/apt-scout __write-harness apply" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Target: refusing /etc/apt/sources.list.d: it is a symlink"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"$evil  /tmp/attacker/evil.sources"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W8 symlinked sources.list.d -> ERR Target, exit 1, attacker's file identical"
else
  fail "W8 symlinked sources.list.d -> exit $rc: $(echo "$got" | head -6)"
fi

# W9 (S6) Offline: the target is substituted, new inode and new bytes, while the run is paused at
#   its last checkpoint, so only the re-check inside `replace_one` can refuse it. The handshake is
#   bounded and its absence fails the case; a harness that exits 97 (never released) is not a pass.
sub=5b68ea6734c4f9888a9d33cdaec832aa50fc5a1cc75e3fe83c3788f99d8a1f6e  # 'substituted-v2\n'
rc=0; got=$(wrun ubuntu:24.04 sh -c "WCASE_DIGESTS=
$WCASE_PRE
$FIX_DEB822
cp /etc/apt/sources.list.d/ubuntu.sources /tmp/pre
snapshot /tmp/before
( /apt-scout __write-harness apply-pause > /tmp/out 2>&1; echo \$? > /tmp/hrc ) &
i=0
while [ ! -e /tmp/apt-scout-harness/paused ] && [ \$i -lt 150 ]; do sleep 0.1; i=\$((i + 1)); done
if [ -e /tmp/apt-scout-harness/paused ]; then echo paused=yes; else echo PAUSE_MARKER_MISSING; fi
mkdir -p /tmp/apt-scout-harness
printf 'substituted-v2\n' > /etc/apt/sources.list.d/.sub
chmod 0644 /etc/apt/sources.list.d/.sub
mv /etc/apt/sources.list.d/.sub /etc/apt/sources.list.d/ubuntu.sources
echo sub=\$(sha256sum < /etc/apt/sources.list.d/ubuntu.sources | cut -d' ' -f1)
printf 'go\n' > /tmp/apt-scout-harness/go
wait
echo rc=\$(cat /tmp/hrc)
snapshot /tmp/after
if cmp -s /tmp/before /tmp/after; then echo same=yes; else echo same=no; fi
diff /tmp/before /tmp/after | grep '^[<>]' > /tmp/dl
echo diff-lines=\$(wc -l < /tmp/dl) others=\$(grep -vc 'sources.list.d/ubuntu.sources' /tmp/dl)
if [ -f /var/backups/apt-scout/20261112T000000Z/sources.list.d/ubuntu.sources ]; then echo backup=yes; else echo backup=no; fi
pre=\$(sha256sum < /tmp/pre | cut -d' ' -f1)
bck=\$(sha256sum < /var/backups/apt-scout/20261112T000000Z/sources.list.d/ubuntu.sources 2>/dev/null | cut -d' ' -f1)
if [ -n \"\$pre\" ] && [ \"\$pre\" = \"\$bck\" ]; then echo backup-copy=same; else echo \"backup-copy=different pre=\$pre bck=\$bck\"; fi
echo tmp=\$(ls -a /etc/apt/sources.list.d | grep -c '\\.apt-scout-tmp-')
cat /tmp/out" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"paused=yes"* ]] && [[ "$got" != *"PAUSE_MARKER_MISSING"* ]] \
  && [[ "$got" == *"rc=1"* ]] && [[ "$got" == *"same=no"* ]] && [[ "$got" != *"diff-lines=0"* ]] \
  && [[ "$got" == *"others=0"* ]] && [[ "$got" == *"sub=$sub"* ]] \
  && [[ "$got" == *"ERR Changed: /etc/apt/sources.list.d/ubuntu.sources changed since the plan was built"* ]] \
  && [[ "$got" == *"backup=yes"* ]] && [[ "$got" == *"backup-copy=same"* ]] && [[ "$got" == *"tmp=0"* ]]; then
  pass "W9 substituted while paused -> ERR Changed, exit 1, substitution kept, backup pre-run"
else
  fail "W9 substituted while paused -> exit $rc: $(echo "$got" | head -12)"
fi

# W10 and W11 are S7 and S8: a read-only rootfs stands in for a permission root would bypass, and
# their /etc/apt layout is the committed $WFIXTURE image, because no setup at all can run against a
# read-only rootfs. Build it here and run both cases through wro(), the only source of those flags.
wfixture

# W10 (S7) Offline, read-only rootfs with a writable /var/backups: the backup phase completes into
#   the tmpfs and the write phase meets EROFS at its first temp file. The second run, in the same
#   UTC second, must refuse the backup directory the first one left: a run that adopted or
#   overwrote it would print neither the Exists line nor backups-identical=yes.
rc=0; got=$(wro --tmpfs /var/backups "$WFIXTURE" sh -c "$WCASE_PRE
snapshot /tmp/before
rc1=0; /apt-scout __write-harness apply > /tmp/out1 2>&1 || rc1=\$?
echo run1-rc=\$rc1
snapshot /tmp/after1
if cmp -s /tmp/before /tmp/after1; then echo run1-same=yes; else echo run1-same=no; fi
echo tmp=\$(ls -a /etc/apt/sources.list.d | grep -c '\.apt-scout-tmp-')
find /var/backups -type f | sort | xargs -r sha256sum > /tmp/bd1
echo backups-before=\$(wc -l < /tmp/bd1)
rc2=0; /apt-scout __write-harness apply > /tmp/out2 2>&1 || rc2=\$?
echo run2-rc=\$rc2
find /var/backups -type f | sort | xargs -r sha256sum > /tmp/bd2
if cmp -s /tmp/bd1 /tmp/bd2; then echo backups-identical=yes; else echo backups-identical=no; fi
snapshot /tmp/after2
if cmp -s /tmp/before /tmp/after2; then echo same=yes; else echo same=no; fi
echo RUN1; cat /tmp/out1
echo RUN2; cat /tmp/out2" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"run1-rc=1"* ]] && [[ "$got" == *"run2-rc=1"* ]] \
  && [[ "$got" == *"ERR Write: cannot write /etc/apt/sources.list.d/.apt-scout-tmp-"* ]] \
  && [[ "$got" == *"Read-only file system"* ]] \
  && [[ "$got" == *"ERR Exists: /var/backups/apt-scout/20261112T000000Z: a backup directory for this UTC second already exists"* ]] \
  && printf '%s\n' "$got" | grep -qx 'run1-same=yes' \
  && printf '%s\n' "$got" | grep -qx 'same=yes' \
  && printf '%s\n' "$got" | grep -qx 'tmp=0' \
  && printf '%s\n' "$got" | grep -qx 'backups-before=2' \
  && printf '%s\n' "$got" | grep -qx 'backups-identical=yes'; then
  pass "W10 read-only /etc/apt -> ERR Write EROFS, then ERR Exists, backup unchanged"
else
  fail "W10 read-only /etc/apt -> exit $rc: $(echo "$got" | head -14)"
fi

# W11 (S8) Offline, read-only rootfs with no writable /var/backups at all: the refusal comes from
#   the backup phase, so it precedes every read of a target and the whole /etc/apt tree, bytes
#   included, is identical afterwards.
rc=0; got=$(wro "$WFIXTURE" sh -c "$WCASE_PRE
snapshot /tmp/before
rc=0; /apt-scout __write-harness apply > /tmp/out 2>&1 || rc=\$?
echo rc=\$rc
snapshot /tmp/after
if cmp -s /tmp/before /tmp/after; then echo same=yes; else echo same=no; fi
if [ -e /var/backups/apt-scout ]; then echo backups=yes; else echo backups=no; fi
cat /tmp/out" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Backup: cannot write the backup of /var/backups/apt-scout: Read-only file system"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W11 read-only /var/backups -> ERR Backup, exit 1, /etc/apt identical"
else
  fail "W11 read-only /var/backups -> exit $rc: $(echo "$got" | head -8)"
fi

# W12, W13 and W14 (S9) Offline: the three pre-created /var/backups/apt-scout states the writer
#   must refuse. Each asserts its own reason, so one refusal cannot stand in for another, and each
#   digests the backup root's contents, so a directory that gains a file shows up as a change.
rc=0; got=$(WCASE_DIGESTS='/var/backups/apt-scout/*' wcase ubuntu:24.04 \
  "$FIX_DEB822; mkdir -p /var/backups/apt-scout; chown 1000:1000 /var/backups/apt-scout; chmod 0700 /var/backups/apt-scout" \
  "/apt-scout __write-harness apply" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Backup: cannot write the backup of /var/backups/apt-scout: it is owned by uid 1000, not uid 0"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"missing /var/backups/apt-scout/*"* ]]; then
  pass "W12 backup root owned by uid 1000 -> ERR Backup, exit 1, directory still empty"
else
  fail "W12 backup root owned by uid 1000 -> exit $rc: $(echo "$got" | head -8)"
fi

rc=0; got=$(WCASE_DIGESTS='/var/backups/apt-scout/*' wcase ubuntu:24.04 \
  "$FIX_DEB822; install -d -m 0777 /var/backups/apt-scout" \
  "/apt-scout __write-harness apply" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Backup: cannot write the backup of /var/backups/apt-scout: its mode is 0o777, not 0o700"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"missing /var/backups/apt-scout/*"* ]]; then
  pass "W13 world-writable backup root -> ERR Backup, exit 1, directory still empty"
else
  fail "W13 world-writable backup root -> exit $rc: $(echo "$got" | head -8)"
fi

rc=0; got=$(WCASE_DIGESTS='/tmp/evil/*' wcase ubuntu:24.04 \
  "$FIX_DEB822; mkdir -p /tmp/evil; ln -s /tmp/evil /var/backups/apt-scout" \
  "/apt-scout __write-harness apply" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Backup: cannot write the backup of /var/backups/apt-scout: it is a symlink"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"missing /tmp/evil/*"* ]]; then
  pass "W14 symlinked backup root -> ERR Backup, exit 1, target directory still empty"
else
  fail "W14 symlinked backup root -> exit $rc: $(echo "$got" | head -8)"
fi

# W15 (S11) Offline: every name's verdict from the real `Target`/`SourceName` parse and
#   `Target::relative`'s path builder. A runtime demonstration, not a regression guard: `plan.rs`'s
#   enum makes an out-of-root target unrepresentable, so the type is the control. The accepted name
#   is in the list, so the UNDER half is printed by a real parse instead of being a format nothing
#   exercises. Nine refused lines with only eight non-empty hostile names is what pins the empty
#   name, which prints as `REFUSED ` and would otherwise be the one entry no line could be named
#   for.
long_name='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.sources'
rc=0; got=$(wcase ubuntu:24.04 "$FIX_DEB822" "/apt-scout __write-harness hostile-names" 2>&1) || rc=$?
refused=$(printf '%s\n' "$got" | grep -c '^REFUSED ' || true)
under=$(printf '%s\n' "$got" | grep -c '^UNDER ' || true)
unrefused=
for name in '..' '../../etc/passwd' '/etc/shadow' 'foo.txt' '.list' '.hidden.sources' 'a/b.list' "$long_name"; do
  printf '%s\n' "$got" | grep -qxF "REFUSED $name" || unrefused="$unrefused '$name'"
done
if [ "$rc" -eq 0 ] && [ -n "$refused" ] && [ "$refused" -eq 9 ] && [ -n "$under" ] && [ "$under" -eq 1 ] \
  && [ -z "$unrefused" ] \
  && [[ "$got" == *"UNDER sources.list.d/custom.sources"* ]] \
  && [[ "$got" != *"UNDER sources.list.d/.."* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W15 hostile names -> $refused refused, $under accepted, /etc/apt identical"
else
  fail "W15 hostile names -> exit $rc refused=$refused under=$under unrefused='$unrefused': $(echo "$got" | head -14)"
fi

# W16 (S15) Offline: the run holds no baseline, so the writer refuses at its freshness check, which
#   precedes the capture of any target and the backup root: /etc/apt is untouched and no backup
#   directory is created. This is the refusal S15 hangs on, not the harness's own earlier failure.
rc=0; got=$(wcase ubuntu:24.04 "$FIX_DEB822" "/apt-scout __write-harness no-baseline" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR NoBaseline: this run has no verified freshness baseline for the mirror it selected"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W16 no baseline -> ERR NoBaseline, exit 1, /etc/apt identical, no backup"
else
  fail "W16 no baseline -> exit $rc: $(echo "$got" | head -8)"
fi

# W17 (S15) Offline: the baseline is MAX_LAG_SECS + 1 ahead of the mirror's proof, so the freshness
#   check's stale branch refuses the run. The lag and the limit are both named, so the case cannot
#   pass on the no-baseline refusal or on a stale check that drifted from the constant.
rc=0; got=$(wcase ubuntu:24.04 "$FIX_DEB822" "/apt-scout __write-harness stale" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Stale: the mirror this run selected, http://mirror.example.invalid/ubuntu, is 86401 seconds behind"* ]] \
  && [[ "$got" == *"past the 86400 second limit"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W17 stale mirror -> ERR Stale, 86401 behind, exit 1, /etc/apt identical"
else
  fail "W17 stale mirror -> exit $rc: $(echo "$got" | head -8)"
fi

# W18 (S15) Offline: a plan whose digest is not the digest of its bytes cannot be built, so the
#   digest rule refuses it before `apply` is reached and nothing on disk is opened for writing.
#   The target is named, so the line is the digest rule's and not another rule's.
rc=0; got=$(wcase ubuntu:24.04 "$FIX_DEB822" "/apt-scout __write-harness digest-mismatch" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Plan: digest rule: the bytes of sources.list.d/ubuntu.sources do not match the recorded digest"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W18 digest does not describe the bytes -> ERR Plan digest rule, exit 1, /etc/apt identical"
else
  fail "W18 digest mismatch -> exit $rc: $(echo "$got" | head -8)"
fi

# W19 (S15) Offline: one body carries a `Date:` line before the armour. The strict read refuses it,
#   the tolerant read returns that date (1794558543, the same forgery the stale case uses), and a
#   mirror whose only date came from outside the signed region has no proof, so its plan is refused.
rc=0; got=$(wcase ubuntu:24.04 "$FIX_DEB822" "/apt-scout __write-harness prefixed" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"STRICT refused: InRelease is empty or does not start with '-----BEGIN PGP SIGNED MESSAGE-----'"* ]] \
  && [[ "$got" == *"TOLERANT 1794558543"* ]] \
  && [[ "$got" == *"ERR Plan: proof rule: the mirror is not verified"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W19 Date: before the armour -> strict refusal, tolerant 1794558543, ERR Plan proof rule"
else
  fail "W19 prefixed body -> exit $rc: $(echo "$got" | head -8)"
fi

# W20 and W21 are S17's pair: `apply-pop` plans the creation of sources.list.d/system-security.sources
# and the replacement of the pinned ubuntu.sources. Both cases make the create target un-creatable,
# and each must be refused by the writer's own variant for that state, with the file's bytes, mode
# and owner unchanged and no backup directory started.

# W20 (S17) Offline: a regular file already sits where the plan creates one, so the capture's
#   `None`-original against a present file is a Changed, not an Exists (only the post-capture
#   O_EXCL race reaches Exists). The harness's replace target is ubuntu.sources, never this file:
#   a plan that selected it as well would be refused as a duplicate before the writer ran.
rc=0; got=$(WCASE_DIGESTS=/etc/apt/sources.list.d/system-security.sources wcase ubuntu:24.04 \
  "$FIX_DEB822; printf 'orig\n' > /etc/apt/sources.list.d/system-security.sources; chmod 0644 /etc/apt/sources.list.d/system-security.sources" \
  "/apt-scout __write-harness apply-pop" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Changed: /etc/apt/sources.list.d/system-security.sources changed since the plan was built: the file on disk is not the file the plan was built from"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"backups=no"* ]]; then
  pass "W20 create target already exists -> ERR Changed, exit 1, file and /etc/apt identical"
else
  fail "W20 create target already exists -> exit $rc: $(echo "$got" | head -8)"
fi

# W21 (S17) Offline: the create target is a symlink with real bytes behind it, so `read_state`'s
#   lstat sees the link and the refusal names the target. The canary's digest is compared, so a
#   writer that followed the link and truncated it would show as digests=changed.
rc=0; got=$(WCASE_DIGESTS=/tmp/canary wcase ubuntu:24.04 \
  "$FIX_DEB822; printf 'canary-v1\n' > /tmp/canary; ln -s /tmp/canary /etc/apt/sources.list.d/system-security.sources" \
  "/apt-scout __write-harness apply-pop" 2>&1) || rc=$?
if [ "$rc" -eq 0 ] && [[ "$got" == *"rc=1"* ]] \
  && [[ "$got" == *"ERR Target: refusing /etc/apt/sources.list.d/system-security.sources: it is a symlink"* ]] \
  && [[ "$got" == *"same=yes"* ]] && [[ "$got" == *"digests=unchanged"* ]] \
  && [[ "$got" == *"$canary  /tmp/canary"* ]] && [[ "$got" == *"backups=no"* ]]; then
  pass "W21 symlinked create target -> ERR Target, exit 1, canary and /etc/apt identical"
else
  fail "W21 symlinked create target -> exit $rc: $(echo "$got" | head -8)"
fi

# W22 Offline: the positive path. Without it every refusal above could pass on a harness broken
#   enough to refuse everything. The target it names is the one $FIX_DEB822 guarantees, the backup
#   holds the pre-run copy and its manifest, and the snapshot diff is exactly two changed lines,
#   both of that one file, so no other /etc/apt path moved.
rc=0; got=$(wcase ubuntu:24.04 "$FIX_DEB822" "/apt-scout __write-harness apply && stat -c 'stat=%a %U:%G' /etc/apt/sources.list.d/ubuntu.sources && grep -q mirror.example.invalid /etc/apt/sources.list.d/ubuntu.sources && echo replaced && find /var/backups/apt-scout -mindepth 1 | sort && grep -q archive.ubuntu.com /var/backups/apt-scout/20261112T000000Z/sources.list.d/ubuntu.sources && echo backup-pre=yes" 2>&1) || rc=$?
diffchanged=$(printf '%s\n' "$got" | grep -cE '^[-+]([fdl] |[0-9a-f]{64}  )' || true)
others=$(printf '%s\n' "$got" | grep -E '^[-+]([fdl] |[0-9a-f]{64}  )' | grep -vc 'sources.list.d/ubuntu.sources' || true)
if [ "$rc" -eq 0 ] && printf '%s\n' "$got" | grep -qx 'rc=0' \
  && printf '%s\n' "$got" | grep -qxF 'OK backup=20261112T000000Z wrote=sources.list.d/ubuntu.sources' \
  && printf '%s\n' "$got" | grep -qxF 'stat=644 root:root' \
  && printf '%s\n' "$got" | grep -qxF 'replaced' \
  && printf '%s\n' "$got" | grep -qxF 'backup-pre=yes' \
  && printf '%s\n' "$got" | grep -qxF '/var/backups/apt-scout/20261112T000000Z/MANIFEST' \
  && printf '%s\n' "$got" | grep -qxF '/var/backups/apt-scout/20261112T000000Z/sources.list.d/ubuntu.sources' \
  && [[ "$got" == *"same=no"* ]] && [[ "$got" == *"backups=yes"* ]] \
  && [[ "$got" != *".apt-scout-tmp-"* ]] \
  && [ -n "$diffchanged" ] && [ "$diffchanged" -eq 2 ] && [ -n "$others" ] && [ "$others" -eq 0 ]; then
  pass "W22 positive write -> OK line, mirror host, backup with manifest, only that file changed"
else
  fail "W22 positive write -> exit $rc diff=$diffchanged others=$others: $(echo "$got" | head -14)"
fi

# W23 Live (needs internet): a mirror this tool wrote survives a country change. One container, one
#   pinned /etc/apt, two runs, and nothing between them edits the file: the first run writes a
#   Norwegian mirror, the second asks for a German one. Shape-tolerant per D29 — it never names the
#   mirror either run picks, only that run 2 recognized the host run 1 wrote (its `Sources` line
#   reads `rewrite` for that host), exited 0, and changed the file. A write needs a verified
#   `-updates`, so the keyring is installed first; a failed install is named as the live-network
#   reason instead of passing quietly.
rc=0; got=$(run bridge ubuntu:24.04 sh -c "$FIX_DEB822
apt-get update -qq >/dev/null 2>&1
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gpgv ubuntu-keyring >/dev/null 2>&1 || { echo DEPENDENCY_INSTALL_FAILED; exit 9; }
rc1=0; /apt-scout --country NO --yes > /tmp/run1 2>&1 || rc1=\$?
echo run1-rc=\$rc1
host=\$(grep -oE 'URIs: [a-z]+://[^ /]+' /etc/apt/sources.list.d/ubuntu.sources | head -n 1 | sed -E 's|^URIs: [a-z]+://||; s|/.*||')
echo run1-host=\$host
before=\$(sha256sum < /etc/apt/sources.list.d/ubuntu.sources | cut -d' ' -f1)
# One second apart, so the two runs cannot land in the same backup second (the writer refuses that).
sleep 1
rc2=0; /apt-scout --country DE --yes > /tmp/run2 2>&1 || rc2=\$?
echo run2-rc=\$rc2
after=\$(sha256sum < /etc/apt/sources.list.d/ubuntu.sources | cut -d' ' -f1)
if [ \"\$before\" = \"\$after\" ]; then echo changed=no; else echo changed=yes; fi
echo RUN1; cat /tmp/run1
echo RUN2; cat /tmp/run2" 2>&1) || rc=$?
run1_host=$(printf '%s\n' "$got" | sed -n 's/^run1-host=//p')
run2_out=${got#*RUN2}
if [[ "$got" == *"DEPENDENCY_INSTALL_FAILED"* ]]; then
  fail "W23 country change -> apt-get install gpgv ubuntu-keyring failed in the container (live-network reason)"
elif [ "$rc" -ne 0 ] || ! printf '%s\n' "$got" | grep -qx 'run1-rc=0'; then
  fail "W23 country change -> run 1 (--country NO) did not write on the live network: $(printf '%s\n' "$got" | head -6)"
elif [ -n "$run1_host" ] && printf '%s\n' "$got" | grep -qx 'run2-rc=0' \
  && [[ "$run2_out" != *"No official Ubuntu apt source URI can be changed"* ]] \
  && printf '%s\n' "$got" | grep -qx 'changed=yes' \
  && printf '%s\n' "$run2_out" | grep -qE "^ +sources\.list\.d/ubuntu\.sources +rewrite  line [0-9]+  ${run1_host//./\\.}$"; then
  pass "W23 a NO mirror stays rewritable under --country DE -> run 2 rewrote $run1_host"
else
  fail "W23 country change -> exit $rc host='$run1_host': $(printf '%s\n' "$got" | head -14)"
fi

# W24 Live: the wider host set never reaches a third-party host or a PPA. Both lines are seeded
#   beside the vendor line, both countries run, and the two seeded lines are byte-identical at the
#   end while run 2's `Sources` section reports each one `skip` with the host it names.
rc=0; got=$(run bridge ubuntu:24.04 sh -c "rm -rf /etc/apt/sources.list.d
printf 'deb http://archive.ubuntu.com/ubuntu noble main universe\\ndeb http://deb.example.com/stable ./\\ndeb https://ppa.launchpadcontent.net/x/y/ubuntu noble main\\n' > /etc/apt/sources.list
chmod 0644 /etc/apt/sources.list
apt-get update -qq >/dev/null 2>&1
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gpgv ubuntu-keyring >/dev/null 2>&1 || { echo DEPENDENCY_INSTALL_FAILED; exit 9; }
seeded() { grep -E '^deb (http://deb\\.example\\.com|https://ppa\\.launchpadcontent\\.net)' /etc/apt/sources.list; }
seeded > /tmp/seeded-before
rc1=0; /apt-scout --country NO --yes > /tmp/run1 2>&1 || rc1=\$?
echo run1-rc=\$rc1
sleep 1
rc2=0; /apt-scout --country DE --yes > /tmp/run2 2>&1 || rc2=\$?
echo run2-rc=\$rc2
seeded > /tmp/seeded-after
if cmp -s /tmp/seeded-before /tmp/seeded-after; then echo seeded=same; else echo seeded=changed; diff /tmp/seeded-before /tmp/seeded-after; fi
echo RUN1; cat /tmp/run1
echo RUN2; cat /tmp/run2" 2>&1) || rc=$?
run2_out=${got#*RUN2}
if [[ "$got" == *"DEPENDENCY_INSTALL_FAILED"* ]]; then
  fail "W24 third-party and PPA lines -> apt-get install gpgv ubuntu-keyring failed in the container (live-network reason)"
elif [ "$rc" -ne 0 ] || ! printf '%s\n' "$got" | grep -qx 'run1-rc=0'; then
  fail "W24 third-party and PPA lines -> run 1 (--country NO) did not write on the live network: $(printf '%s\n' "$got" | head -6)"
elif printf '%s\n' "$got" | grep -qx 'run2-rc=0' \
  && printf '%s\n' "$got" | grep -qx 'seeded=same' \
  && printf '%s\n' "$run2_out" | grep -qE '^ +sources\.list +skip +line [0-9]+  deb\.example\.com — ' \
  && printf '%s\n' "$run2_out" | grep -qE '^ +sources\.list +skip +line [0-9]+  ppa\.launchpadcontent\.net — '; then
  pass "W24 third-party and PPA lines survive two country changes and are reported skip"
else
  fail "W24 third-party and PPA lines -> exit $rc: $(printf '%s\n' "$got" | head -14)"
fi

# W25 Live (needs internet): the one automatic widening. Nothing about the page is hard-coded: the
#   container learns this country's own hosts from the run's own `Ranked` table, maps every one of
#   them to 127.0.0.1 in /etc/hosts (the resolver drops loopback addresses, so each fails fast
#   without reaching the network), and then runs the same country for real. With the country's whole
#   set unusable the run must widen once, say so on its own line, and write a mirror from outside
#   that set — a run that did not widen fails at `no mirror passed the freshness and reachability
#   checks` and writes nothing. Neither run's mirror is named.
rc=0; got=$(run bridge ubuntu:24.04 sh -c "$FIX_DEB822
apt-get update -qq >/dev/null 2>&1
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gpgv ubuntu-keyring >/dev/null 2>&1 || { echo DEPENDENCY_INSTALL_FAILED; exit 9; }
ranked_hosts() { awk '/^Ranked/{f=1;next} f&&/^[[:space:]]*\$/{exit} f' \"\$1\" | grep -oE 'https?://[^/ ]+' | sed -E 's|^https?://||' | sort -u; }
/apt-scout --list --country LT --top 30 --verbose > /tmp/list1 2>&1 || { echo LIST_FAILED; exit 8; }
ranked_hosts /tmp/list1 > /tmp/lt-hosts
echo blocked=\$(wc -l < /tmp/lt-hosts)
[ -s /tmp/lt-hosts ] || { echo NO_HOSTS; exit 7; }
while read -r h; do printf '127.0.0.1 %s\n' \"\$h\" >> /etc/hosts; done < /tmp/lt-hosts
sleep 1
rc1=0; /apt-scout --country LT --yes > /tmp/run1 2>&1 || rc1=\$?
echo run1-rc=\$rc1
host=\$(grep -oE 'URIs: [a-z]+://[^ /]+' /etc/apt/sources.list.d/ubuntu.sources | head -n 1 | sed -E 's|^URIs: [a-z]+://||; s|/.*||')
echo written-host=\$host
# The recommendation names the mirror's country, not the run's: this run asked for LT and selected
# a foreign mirror, so a line that still says Lithuania is the bug SPEC.md CLI forbids.
echo rec=\$(grep -E '— (freshest|closest) ' /tmp/run1 | head -n 1)
echo BLOCKED-BEGIN; cat /tmp/lt-hosts; echo BLOCKED-END
echo RUN1; cat /tmp/run1" 2>&1) || rc=$?
written=$(printf '%s\n' "$got" | sed -n 's/^written-host=//p')
blocked=$(printf '%s\n' "$got" | sed -n '/^BLOCKED-BEGIN$/,/^BLOCKED-END$/p' | grep -v '^BLOCKED-')
run1_out=$(printf '%s\n' "$got" | sed -n '/^RUN1$/,$p')
if [[ "$got" == *"DEPENDENCY_INSTALL_FAILED"* ]]; then
  fail "W25 widening -> apt-get install gpgv ubuntu-keyring failed in the container (live-network reason)"
elif [[ "$got" == *"NO_HOSTS"* || "$got" == *"LIST_FAILED"* ]]; then
  fail "W25 widening -> the learning pass (--list --country LT) named no ranked host: $(printf '%s\n' "$got" | head -6)"
elif [ "$rc" -ne 0 ] || ! printf '%s\n' "$got" | grep -qx 'run1-rc=0'; then
  fail "W25 widening -> the country's own set blocked and the run did not recover: $(printf '%s\n' "$got" | head -8)"
elif [ -n "$written" ] && [ -n "$blocked" ] \
  && printf '%s\n' "$blocked" | grep -qvxF "$written" \
  && printf '%s\n' "$run1_out" | grep -qE '^note: no usable mirror in ' \
  && printf '%s\n' "$run1_out" | grep -qF 'note: no usable mirror in Lithuania (LT)' \
  && rec=$(printf '%s\n' "$got" | sed -n 's/^rec=//p') && [ -n "$rec" ] && [[ "$rec" != *Lithuania* ]]; then
  pass "W25 widening -> every LT mirror blocked, a foreign mirror ($written) written, the note says why"
else
  fail "W25 widening -> written='$written' blocked=$(printf '%s\n' "$blocked" | wc -l): $(printf '%s\n' "$run1_out" | head -14)"
fi

# W26 Live (needs internet): `--any` makes the country display-only. The same learning pass gives the
#   country's own host set; a `--any` run then has to say in its header that the country is not a
#   constraint, rank at least one host from outside that set, and print no widening line — it never
#   widens, because its set already is every country. Which mirror it ranks first is never asserted.
rc=0; got=$(run bridge ubuntu:24.04 sh -c "$FIX_DEB822
apt-get update -qq >/dev/null 2>&1
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gpgv ubuntu-keyring >/dev/null 2>&1 || { echo DEPENDENCY_INSTALL_FAILED; exit 9; }
ranked_hosts() { awk '/^Ranked/{f=1;next} f&&/^[[:space:]]*\$/{exit} f' \"\$1\" | grep -oE 'https?://[^/ ]+' | sed -E 's|^https?://||' | sort -u; }
/apt-scout --list --country LT --top 30 --verbose > /tmp/list1 2>&1 || { echo LIST_FAILED; exit 8; }
ranked_hosts /tmp/list1 > /tmp/lt-hosts
echo lt-hosts=\$(wc -l < /tmp/lt-hosts)
[ -s /tmp/lt-hosts ] || { echo NO_HOSTS; exit 7; }
rc2=0; /apt-scout --list --any --country LT --top 30 --verbose > /tmp/run2 2>&1 || rc2=\$?
echo run2-rc=\$rc2
ranked_hosts /tmp/run2 > /tmp/any-hosts
echo any-hosts=\$(wc -l < /tmp/any-hosts)
echo outside=\$(comm -13 /tmp/lt-hosts /tmp/any-hosts | wc -l)
echo RUN2; cat /tmp/run2" 2>&1) || rc=$?
run2_out=$(printf '%s\n' "$got" | sed -n '/^RUN2$/,$p')
outside=$(printf '%s\n' "$got" | sed -n 's/^outside=//p')
lt_hosts=$(printf '%s\n' "$got" | sed -n 's/^lt-hosts=//p')
if [[ "$got" == *"DEPENDENCY_INSTALL_FAILED"* ]]; then
  fail "W26 --any -> apt-get install gpgv ubuntu-keyring failed in the container (live-network reason)"
elif [[ "$got" == *"NO_HOSTS"* || "$got" == *"LIST_FAILED"* ]]; then
  fail "W26 --any -> the learning pass (--list --country LT) named no ranked host: $(printf '%s\n' "$got" | head -6)"
elif [ "$rc" -ne 0 ] || ! printf '%s\n' "$got" | grep -qx 'run2-rc=0'; then
  fail "W26 --any -> the flag was refused or the run failed: $(printf '%s\n' "$got" | head -8)"
elif printf '%s\n' "$run2_out" | grep -qF 'country: Lithuania (--country, not a constraint)' \
  && [ -n "$outside" ] && [ "$outside" -gt 0 ] \
  && ! printf '%s\n' "$run2_out" | grep -qE '^note: no usable mirror in '; then
  pass "W26 --any -> country not a constraint, $outside of the ranked hosts outside Lithuania's own set, no widening note"
else
  fail "W26 --any -> outside=$outside lt-hosts=$lt_hosts: $(printf '%s\n' "$run2_out" | head -14)"
fi

exit $status
