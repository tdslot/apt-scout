# Freshness is the `-updates` `Date`, compared exactly, with a 24-hour lag limit

A mirror's freshness is decided by `<codename>-updates` alone.
Its `InRelease` `Date` is compared **exactly** with the reference — no tolerance — because a tolerance would
silently accept a mirror one signing generation behind a security upload.
A mirror is stale, and rejected, when its `Date` is older than the reference by more than `MAX_LAG` (24 hours); lagging
but fresh mirrors rank lower.
The release pocket is not compared, and `-security` lag is informational.

Signature status is deliberately **not** a ranking input: a mirror whose `InRelease` is unsigned or signed by an unknown
key may still rank first, because ranking answers "which mirror is fastest and current", and a write has a separate gate
that requires `verified`.

**Consequences**: a slow mirror with the exact reference `Date` outranks a much faster one that is one signing
generation behind.
That is the intended trade-off — freshness before speed — and the ranked table shows both facts so a person can
overrule it.

**Spec**: `docs/spec/pipeline.md`. **Aliases**: none.
