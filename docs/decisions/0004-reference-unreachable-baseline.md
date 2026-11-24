# When the reference host is unreachable, use the newest candidate `Date` as the baseline

The freshness baseline is the `InRelease` of `archive.ubuntu.com` for the same pocket.
If that host is unreachable (network error, 5xx), the run uses the newest `Date` among the candidates as the baseline
and prints a warning naming that choice.
The local clock is never the baseline: a machine with a wrong clock must not be able to make every other mirror look
stale.

The fallback is not used when the reference answers 404 for the codename: that means the release has left the archive,
and the run stops with a message naming the codename instead of inventing a baseline.

**Spec**: `docs/spec/pipeline.md`. **Aliases**: none.
