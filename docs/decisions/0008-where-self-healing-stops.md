# The run heals what it can, and knows where to stop

A repair that guesses is worse than a failure that explains itself: the expensive mistakes are the ones that write
something plausible over something correct. The self-healing the run does is therefore limited to steps whose
correct answer needs no judgement, and each boundary below is a decision rather than a lack of effort.

## Why v1 keeps no cache

The mirror list, the geolocation answers and the AS lookups are fetched once per run. A cache would make a stale
answer look like a current one at exactly the moment the run is deciding which mirror is fresh, and the run takes
seconds by design: [ADR-0002](0002-mirror-list-source-order.md) records the cache as a planned source behind the live
ones, which is the shape that cannot shadow a live answer.

## Why the distribution is read, never asked for

The codename comes from `os-release`, `/etc/os-release` first and `/usr/lib/os-release` second. There is no
`lsb_release` path, because that command is absent from minimal images and says nothing the two files do not; and
there is no `--codename` or `--distro` flag, because a flag is a second source of truth that can disagree with the
system the run is about to write to, and the disagreement would be silent.

## Why `UBUNTU_CODENAME` wins over `VERSION_CODENAME`

A derivative sets `VERSION_CODENAME` to its own release name: Linux Mint reports `wilma`, while its apt
sources use Ubuntu's `noble` suites, so a run that trusted the version codename would plan suites that do
not exist on the mirror. `UBUNTU_CODENAME` is the field the derivative itself publishes as its Ubuntu base, so it
is read first and the fallback is only for a file that omits it.

## Why the codename is never guessed

The apt sources name suites, and a suite usually contains a codename, so guessing is possible and was rejected: a
derivative renames suites, a pin or a third-party repository adds suites that belong to no Ubuntu release, and a
wrong codename plans a wrong suite. Both files unreadable therefore ends the run instead of continuing on a guess.

## Where self-healing stops

A failed backup ends the run, and a self-heal never creates a file in `/etc/apt`, because the safe answer to an
unfamiliar apt layout is to report it. The one file apt-scout creates there is not a repair: it is the planned
`sources.list.d/system-security.sources` on Pop!_OS, which comes from the validated plan and is opened with
`create_new`, so it can neither overwrite nor be overwritten.

**Spec**: `docs/spec/errors.md`, `ERR-3` (what is repaired), `ERR-4` (no cache) and `ERR-5` (where it
  stops), and `docs/spec/platform.md`, `PLAT-1` (which codename wins). **Aliases**: none.
