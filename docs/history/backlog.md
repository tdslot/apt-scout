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
