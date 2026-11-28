# Decisions

One file per decision that the specification does not explain by itself, plus the map of the ids that live elsewhere.

A decision record answers "why this way, and what else was considered".
It is not a requirement: `docs/spec/` says what apt-scout must do, and where the two disagree the requirement wins.
Evidence about the current code may be cited as `path:line` and is allowed to age.

## Recorded here

| ADR | Decision |
| --- | --- |
| [0001](0001-pop-os-security-suite-split.md) | Split Pop!_OS `-security` into its own sources file |
| [0002](0002-mirror-list-source-order.md) | Source the mirror list from Launchpad first and mirrors.ubuntu.com second |
| [0003](0003-freshness-and-ranking-policy.md) | Freshness is the `-updates` `Date`, compared exactly, with a 24-hour lag limit |
| [0004](0004-reference-unreachable-baseline.md) | When the reference host is unreachable, use the newest candidate `Date` as the baseline |
| [0005](0005-mirror-list-and-freshness-measurements.md) | The measurements the mirror-list parsers and `MAX_LAG` rest on, with their dates |
| [0006](0006-what-the-writer-refuses.md) | What the writer refuses, and why each refusal is explicit |
| [0007](0007-the-reports-two-faces.md) | The report serves a person and a harness, and why the styling rules follow |
| [0008](0008-where-self-healing-stops.md) | The run heals what it can, and knows where to stop |
| [0009](0009-exit-codes-and-the-date-boundary.md) | Why the exit codes are five, and why a date can rank but never install |
| [0010](0010-why-the-locale-layer-is-shaped-this-way.md) | Why the locale layer is shaped this way |

## Recorded elsewhere

The privilege boundary is recorded as one document, `docs/threat-model.md`, carrying its ids in the title
and in section 7. Those ids are not repeated here.

| Ids | Where |
| --- | --- |
| D6c, D6e, D6f, D9, D9a, D9c, D10, D10b, D11, D11a, D11b, D12, D13, D25, D30a, D30c, D31a, D42, D49 | `docs/threat-model.md` |

`ROADMAP.md` records the task each id became, its state, the pull request that closed it and the evidence, with the
gate records archived under `docs/history/`.
An id that names a task rather than a decision is not a decision record, and an id whose substance is a ruling in the
threat model does not get a second file.

## Numbering

Four digits, one sequence, never reused. A new record scans this directory for the highest number and increments it.
