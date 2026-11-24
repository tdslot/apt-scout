# Backlog history

Moved out of `ROADMAP.md` by D84: the evidence behind each task, kept verbatim.
The ledger holds the claim and the state; this file holds the gates, the verdicts and the logs.

### D76b

- [x] D76b. Give the requirements the normative voice: `MUST`/`SHOULD`/`MAY` per requirement, which means judging each of the 117 ids and rewording only where the current sentence does not already state an obligation. Done in this branch on `docs/normative-voice`: 117/117 ids carry at least one keyword on the line that names them; `SPEC.md`'s index was regenerated from those sentences and a verification loop proves every row occurs verbatim in its chapter; the counts hold at 32 planned, 6 partial, 79 implemented. `SHOULD` is used twice (`NFR-6` layout, `TUI-7` step count) because those are the two preferences between valid outcomes the text already stated. `NFR-5`'s milestone list — PR numbers and completion states — is deleted and the id now carries the rule that the sequence is `ROADMAP.md`'s. Two `docs/spec/AGENTS.md` P2s are closed: the lessons file is named as machine-level rather than at `.pi/agents/lessons.md`, and the id check is a set diff plus an occurrence count with what each proves. [SPEC: docs/spec/AGENTS.md]

### D89

The brief, kept from `docs/tasks/D89.md` before D85's rule deleted it, and what the six review rounds added.

- The four paths B7 names are escaped at the boundary that owns each string, not at the print line, so the run's own
  colour escapes are not rewritten: the report (`Report::print`), the closing line inside `write_closing_block`,
  `rejected_note` plus the note that prints it, and the `error:` line beside the two warnings that quote a URL.
- Paths beyond the four the audit found: strings built from a URI that the tool formats itself — the AS
  note, the planned change diff, the fallback warning, the inventory rows, the AS-lookup note. All escaped at
  `58a6d02` and `4ad5413`.
- The escape covers every Unicode `Cc` character, not ASCII alone: `\u{9b}` and `\u{85}` are control characters that
  a UTF-8 terminal decodes, and the review caught the first version rewriting only `0x00-0x1f`.
- Column padding sits on the *escaped* value (`{:<40}` on `control_free(&path)`), because `\x1b` is five characters
  and would shift the status column if the padding went on the raw one.
- The container half did not land: reaching a mirror string end to end needs a list source that can be injected, and
  none exists. That is D90. D89 stays PARTIAL rather than claiming the record's acceptance is met.
- The audit found a broken merge gate on the way: `scripts/docker-test.sh` case 14 asserted `NAME`, `SYNOPSIS` and the
  NAME line while the container wrote the page to `/tmp/page` and printed only its exit code, so the case could never
  pass. Fixed in the same PR, verified by running that exact docker command, and case 16 no longer aborts the whole
  run when the page's flag list comes back empty.
- Not covered, opened as D91: no test pins the escapes in `sources_section`, the geolocation note, the fallback
  warning or the top-level error line. Deleting them leaves every test green.

PR #121, squash `e55a426`. Gates at the final head `4ad5413`: `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test --locked` 320 + 3, `cargo deny check`, six green checks on that head, a reviewer verdict of OK and a
security verdict of CLEAR, both naming that head.

### D91

The brief, kept from `docs/tasks/D91.md` before D85's rule deleted it, and what the work found.

- Four sites escaped correctly and no test pinned them: the sources inventory section, the geolocation note, the
  fallback list warning and `main`'s `error:` line. Deleting any escape left the suite green.
- Three of them printed from inside `eprintln!` in a function that also did other work, so the escape could not be
  called from a test at all. Each became a builder returning the line: `geolocation_note`, `fallback_warning`,
  `error_line`. That is the durable part of this task: not the assertions, but moving the print text into a function
  a test can reach.
- The sources-section test needed a fixture whose *path* and whose per-entry reason carry a control character, which
  is what makes each of its five escaping rows fail when its own escape is removed; the sixth row, an empty
  inventory, interpolates nothing. The first version of the fixture put the control character only in the
  file-level issue, so three of the four escape calls could be deleted with the suite still green — found by
  mutating each one, not by reading the test.
- `TEST-9` states the guard: a test counts the `escape::control_free` call sites in `src/main.rs` and fails when the
  count moves, so a new site is red until someone covers it or records why it carries no foreign text. The needle is
  built with `concat!` so the test does not count its own source line.
- Nine mutations were run and each failed the suite: the four builders' escapes, the file-level issue row, the
  no-active-entries row, the empty-uri row, the rewrite row's path and host, and the skip row's reason.
  The no-active-entries row was missed by the first fixture and caught by the review, which proved it with a
  count-preserving mutation: the call stays and the escape is undone, so the counting test cannot see it.

PR #123. Gates at the head: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --locked` 325 + 3,
`cargo deny check`.

### D90

The brief, kept from `docs/tasks/D90.md` before D85's rule deleted it, and what building it settled.

- The seam is a second `#[cfg(apt_scout_write_harness)]` case, `__write-harness escaped-list`. It serves a
  Launchpad page from a constant compiled into the binary, runs the real `load_candidates` against it, and prints
  what the run says about it. No environment read, no path argument: the harness doctrine in `src/harness.rs` and
  S16 of the record already required exactly that, so the fourth environment read the brief warned about never
  became necessary.
- What made it possible was noticing that `load_candidates` already takes `&dyn Fetch`, so the harness needed a
  `Fetch` implementation and nothing else; `LAUNCHPAD` and `load_candidates` became `pub(crate)`, which is the whole
  of the production change. `http::Canned` stayed test-only.
- The list carries `http://m.example/ubuntu/\x1b[2J`. `mirrors::validate_url` rejects that URL, so it is never
  probed and never written; it reaches the rejection note, which is a print site, and the case asserts the note
  prints `\x1b[2J` as text. This is why the case proves the escaping rather than the rejection, and the spec now
  says so.
- Case 17 of `scripts/docker-test.sh` runs it in `ubuntu:24.04` with `--network none` and fails on a single raw
  control byte. Mutating `rejected_note` to return each URL unmapped made the case find exactly one, which is the
  half a unit test cannot show: that a real terminal-less container receives no escape sequence.
- D89 moved from PARTIAL to DONE with this, because its missing acceptance was exactly this case.

PR #125. Gates at the head: `cargo fmt --check`, `cargo clippy -D warnings` in both the plain and the cfg build,
`cargo test --locked` 325 + 3, `cargo deny check`, and `scripts/docker-test.sh --build-only` plus the case itself.

### D92

The brief, kept from `docs/tasks/D92.md` before D85's rule deleted it, and what the work found.

- Six sites were escaped and unpinned: `plan_changes` printed the new file's bytes and both of its URI lines, and
  `run` printed the Team Cymru failure, the reference-read warning and the `nothing to change` line with the best URL.
  The counting test in D91 cannot see any of them, because a call that stays in place with its escape undone keeps the
  count.
- The same move as D91 was needed first: five builders, `as_lookup_note`, `reference_warning`, `nothing_to_change_line`,
  `added_line` and `removed_line`. Until the print text lives in a function, no test can reach a print that happens
  inside `run`.
- Five mutations were run and each failed the test that covers it. The first attempt filtered on test names containing
  `escapes`, which excluded the new test — three mutations looked green until the filter was fixed. A mutation that
  passes with the wrong filter is the same false confidence the task exists to remove.
- `ESCAPE_SITES` moved from 37 to 36, not up: two call sites in `plan_changes` became one builder each.
- The review found a regression the gates could not: `reference_warning` split its literal across two source lines
  without a continuation, so the compiled string gained a newline and the warning printed as two stderr lines.
  `cargo fmt`, clippy and the tests all passed, because `assert_no_control` flattens newlines before checking.
  The literal is one line again and the test now asserts the warning holds no newline, a mutation of which fails it.
- `TEST-9` and `OUT-12` are `implemented` now, and no `partial:` row in `SPEC.md` mentions the escaping any more.

PR #127. Gates at the head: `cargo fmt --check`, `cargo clippy -D warnings` in both builds, `cargo test --locked`
327 + 3, `cargo deny check`.

### D83 — first slice

The ledger row carries the claim and the state; this records the first slice of D83 and what it settled.

- `docs/spec/pipeline.md` no longer carries its measurements. Four moved to
  `docs/decisions/0005-mirror-list-and-freshness-measurements.md`: the Launchpad page shape and counts,
  measured 2026-11-13; the `mirrors.ubuntu.com` country file, measured 2026-11-13; the `InRelease` size of
  about 130 KB, which is an approximation and is labelled one; and the 2026-11-11 freshness measurement across
  10 Lithuanian and Norwegian mirrors that decided `MAX_LAG`.
- Each requirement that rested on one now points at the record or at the rule that owns it: steps 3 and 7 cite
  0005, steps 6 and 8 cite 0003, and the `-security` note points at the refusal list in `apply.md`,
  which is what actually keeps that host unwritten.
- The 20% same-AS band was described in the chapter as measured. It is not: it is a chosen constant in
  `src/fresh.rs`. Writing the citation forced the check, and the requirement now says it is a choice
  instead of pointing at a record that could not have supported it.
- The chapter's reworded PIPE-9 sentence and the `SPEC.md` index cell drifted apart, which one of the
  traceability loops published in `docs/spec/AGENTS.md` caught: the cell must carry the chapter's first
  sentence verbatim. Replaying that loop against `origin/develop` also showed two older cells (TEST-9,
  TEST-10) written as truncated sentences that ended on a full stop the chapter does not have, so both were
  completed in this pass rather than left as known failures.
- The four older decision records were reflowed to one sentence per line under 120 bytes and are word for word
  identical to the text before, checked whitespace-insensitively. `docs/decisions/README.md` was reflowed
  too, but it also gained the 0005 row, so it is the one file whose words changed. The rule in `docs/AGENTS.md`
  governs new text and grandfathers old, so that reflow was tidying rather than compliance, and plenty of other
  tracked files still carry long lines.
- Two things this branch broke and then repaired, both caught by review: replacing the D83 row swallowed the
  ledger's last section, so the `## Change log` pointer is restored; and the list of chapters left to move
  omitted `testing.md`, which carries two dated observations and five rationale markers of its own.

What remains for D83: `apply.md` and `output.md`, then `cli.md`, `errors.md`,
  `localization.md`, `testing.md`, `non-functional.md` and `platform.md`;
  `tui.md` carries nothing to move.

### D83 — second slice

The ledger row carries the claim and the state; this records the second slice of D83 and what it settled.

- `docs/spec/apply.md` and `docs/spec/output.md` no longer carry their design reasoning. Two records hold it:
  `docs/decisions/0006-what-the-writer-refuses.md` and `docs/decisions/0007-the-reports-two-faces.md`.
  The first keeps the baseline the writer cannot fetch for itself, the second restore refused by the disk, the
  explicit refusal list, the accepted cost of a path prefix and candidate hygiene; the second keeps why the accent
  carries weight, why the copied line is never styled, why the fallback is named and why the plain output is the
  contract.
- The clauses left in place are consequences of another requirement rather than rationale: a creation needs a
  platform, an id sorts by age because it is a timestamp, a skip is reported rather than forced. None points at a
  record because none is a decision.
- The D83 row first claimed that every requirement in the two chapters points at a record; ten of twenty-eight do.
  The row now names those ten instead, because a set a script prints survives review and a universal does not.
  Three of them carried a reason no record held, so the reason moved with the citation: the note about a listed
  entry whose path is `/` and the ordering of backup ids are sections of ADR-0006, and the harness
  argument in `OUT-7` was already in ADR-0007.
  A fourth claim was false the other way: ADR-0006 described the alternatives mechanism as shipped when
  `APPLY-7` is still planned, and the record now says so in its first words.
- Rewording two first sentences broke their `SPEC.md` cells: `APPLY-14` and `OUT-6` lost the code spans their
  chapters carry. The documented traceability loop did NOT catch it and cannot: it strips backticks on both
  sides, so a cell missing a span still matches. A character-exact comparison over all 120 rows found both, and
  the review of the first attempt is what prompted that comparison.
  `docs/spec/AGENTS.md` now states that limit next to the loop, so the next reader does not trust it for more.
- The chapter citations read `(ADR-0006)` and `(ADR-0007)` rather than relative links, matching how the
  chapters already cite `decision 0003`, and because a link written for `docs/spec/` cannot also appear
  verbatim in a `SPEC.md` cell without breaking one of the two paths.

What remains for D83: `cli.md`, `errors.md`, `localization.md`, `testing.md`,
  `non-functional.md`, `platform.md` and `tui.md`.

### D83 — third slice

The ledger row carries the claim and the state; this records the third slice of D83 and what it settled.

- The `errors.md` slice moved four reasons into
  `docs/decisions/0008-where-self-healing-stops.md`: why v1 keeps no cache, why the distribution is read
  rather than asked for, why the codename is never guessed, and where self-healing stops. The mirror-list order in
  `ERR-3` was already recorded in `docs/decisions/0002-mirror-list-source-order.md` and now cites it instead of
  restating it. The first sentence of `ERR-4` lost its "so there is none to heal", so its `SPEC.md` cell moved
  with it: the character-exact comparison is what catches a stale cell, not the loop.

What remains for D83: `cli.md`, `localization.md`, `testing.md`,
  `non-functional.md`, `platform.md` and `tui.md`.
