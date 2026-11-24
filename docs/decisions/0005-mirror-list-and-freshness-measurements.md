# Mirror-list and freshness measurements the parsers depend on

The numbers below are evidence, not requirements. `docs/spec/pipeline.md` states what each parser must
accept; this record keeps the measurements that make those rules testable, so a later reader can tell a
measured fact from a design choice. Each measurement names its date, because a page or a file that changes
shape invalidates the rule it justified and the parser's fixture has to move with it.

## Launchpad's archive-mirrors page

Measured 2026-11-13 against `https://launchpad.net/ubuntu/+archivemirrors`.

| Fact | Value |
| --- | --- |
| Page size | about 316 KB |
| Country sections | 82 |
| Mirror rows | 562 |
| Statuses seen | `UP` and `UNKNOWN` only |
| Declared speeds | `128 Kbps` to `100 Gbps` |
| Rows with only `ftp` or `rsync` links | 2 |

The structure: a `<tr class="head">` row carries `<th colspan="2">Country</th>` and starts a
country section; a mirror row carries a `/ubuntu/+mirror/<id>` link, one `<a href>` per protocol, a
declared speed, and a `<span class="distromirrorstatus{...}">` status.

What the measurement is for: the two `ftp`/`rsync`-only rows are why the parser must tolerate a mirror
with no `http(s)` protocol instead of assuming one, and the two statuses seen are why an unknown status
must parse rather than fail. The fixture the parser is tested against is a saved copy of this page, so a
change in the page and a change in the test are the same change.

## The `mirrors.ubuntu.com` country file

One file per country, `<CC>.txt`, plain text, one URL per line, already ordered by geographic proximity,
about 6 URLs, with no speed and no status field.

Measured 2026-11-13: `<CC>.txt` for a country with no mirrors of its own holds a single line,
`http://archive.ubuntu.com/ubuntu/`. That is why the fallback is not a wider source: it cannot make a
country's set larger, and no run fetches another country's file.

## Why throughput is measured on `Packages.xz`

A mirror's `-updates` `InRelease` is about 130 KB, which is too small and too variable to measure
throughput: the connection setup dominates the sample. Throughput is therefore measured on a 1 MiB
range of `main/binary-<arch>/Packages.xz`, while time to first byte is measured on the `InRelease`
request itself.

## The 2026-11-11 freshness measurement behind `MAX_LAG`

Measured on 2026-11-11 for `noble-updates`, across 10 Lithuanian and Norwegian mirrors: 6 matched the
reference `Date` exactly, and 4 were identical to each other but one signing generation behind, about 6
hours. A 1-hour limit would therefore have rejected 40% of good mirrors, which is why `MAX_LAG` is 24
hours rather than a tight bound.

That number is the input to decision 0003, not a tolerance: the comparison against the reference stays
exact, and `MAX_LAG` only decides what counts as stale.

**Spec**: `docs/spec/pipeline.md`, steps 3, 7 and 8. **Aliases**: none.
