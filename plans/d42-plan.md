# D42 — progress, colour, and three removed lines

Base: `origin/develop` `029672b`. SPEC: **Output** (whole section), **CLI** (the `--no-color`
row, the short-form sentence, the write-confirmation bullet). Threat record: Appendix A item 14.
Plan owner: architect. Writer: apt-scout-coder. No new dependency, no new module, no build change.

## Scope

1. A transient progress line on stderr, only when stderr is a terminal, ending with
   `done — <usable> usable, <rejected> rejected in <t> s`.
2. Colour in the report: on only when stdout is a terminal, `NO_COLOR` is unset, and
   `--no-color` was not given.
3. Delete three lines: `Rejected — 0`, `  keep as is  …`, `  alternatives: none …`.

## Out of scope

The report's content and section order beyond those deletions (D41 owns the `--help` walk and
appends to SPEC Output if it lands second); `src/apply.rs` and every written byte; exit codes;
`--help`'s prose beyond the new flag line; the `sudo apt-scout …` line (never styled); the TUI;
geolocation, which is not one of the four named phases.

## Files

`src/main.rs` only, plus this plan. `scripts/docker-test.sh` is run, not edited.

## Where the code lives — both decisions in `src/main.rs`, beside `Report`

`Report` (`src/main.rs:544–585`) already holds the display layer, so both new pieces sit next to it
instead of in a new module: a new module would need a `SPEC.md` layout sentence and a root
`AGENTS.md` bullet, and neither buys a capability.

```rust
#[derive(Clone, Copy)]
struct Style { colour: bool }
impl Style {
    fn plain() -> Self;                                  // tests: no escapes, ever
    fn paint(&self, s: &str, code: &str) -> String;       // plain() → s.to_string()
    fn bold/green/yellow/red/dim(&self, s: &str) -> String;
}

/// SPEC Output: all three inputs must agree. Pure; no IO, no terminal library.
fn colour_enabled(stdout_is_terminal: bool, no_color_flag: bool, no_color_set: bool) -> bool;

/// The only environment read colour makes, next to `private_temp_dir`'s `std::env::temp_dir()`
/// (src/verify.rs:54). `var_os`, so a non-UTF-8 value still counts as "set".
fn no_color_from_env() -> bool;

trait Sink { fn rewrite(&mut self, line: &str); fn clear(&mut self); }
struct StderrSink;                                       // `\r` + line + `\x1b[K`; `\r` + `\x1b[K`
struct RecordingSink { lines: Vec<String>, clears: usize }   // tests only

/// SPEC Output. `Progress<S>` is inert when `enabled` is false: every method returns at once, so
/// the no-terminal case is the same code path with an empty sink.
struct Progress<S: Sink> { sink: S, enabled: bool, started: Instant }

/// Pure: the one line that survives. `{:.1}` on `elapsed.as_secs_f64()`.
fn done_line(usable: usize, rejected: usize, elapsed: Duration) -> String;
```

`stderr.is_terminal()` / `stdout.is_terminal()` (`std::io::IsTerminal`, stable since 1.70) are read
once in `run()` and passed in as bools. No crate, no `ioctl`, no `TTY` global.

## Ordered steps

1. **Style.** Add `Style`, `colour_enabled`, `no_color_from_env`; `Args` gains `no_color`; the
   parser accepts `--no-color` (long only) and `HELP` gains one line for it; `run()` builds
   `Style` **once**, after `parse_args` and before anything is printed.
2. **Paint the report.** Every builder that emits a styled item takes `&Style`; `Style::plain()`
   returns the input unchanged, so the plain bytes are exactly today's bytes.
   Sites: `header_identity` (bold), `titled`'s left part and the bare headings `Recommendation`
   (`:1352`), `Rejected — n` (`:883`, `:892`) and `Detail` (`:921`) (bold), the `sig` cell in
   `ranked_rows` (`:702`) `ok` green / `no`, `unavailable` yellow, `plan_changes`' `- ` red and
   `+ ` green (`:1073`, `:1074`), `rejected_section`/`rejected_detail` item text red
   (`:880–905`), the header words `lag`, `speed`, `delay` dim (`:676`), the selected URL bold in
   `recommendation` (`:797`), the prompt bold at its one call site (`:1462`).
   **Rule: paint a finished, padded cell.** `format!("{:<44.44}")` counts bytes, so styling before
   a width format breaks every column. Where a value is padded, build it, then paint it.
3. **Progress.** Add `Sink`/`Progress`/`done_line`; construct immediately before
   `load_candidates` (`:1144`) so the elapsed time spans exactly the four named phases; then
   `progress.load(…)` after the list loads, `progress.reference()` around `probe::fetch_reference`
   (`:1190`), `progress.probe(done, total)` inside the join loop (`:1223`, `done` = handles joined
   so far), `progress.ranked()` before the rows are built (`:1336`), and
   `progress.finish(rows.len(), rejected.len())` immediately **before every `report.print()`
   (`:1186`, `:1347`, `:1431`)**, so no transient line survives into the report. The no-fresh
   path finishes with `0` usable.
4. **Warnings keep their own line.** The reference warning (`:1201`) is the one stderr write that
   can land while a progress line is live: call `progress.clear()` immediately before it.
5. **The three deletions.** `rejected_lines` returns an empty `Vec` when `rejected` is empty
   (so `Report::block`, which already skips an empty block, prints no heading); delete the
   `change.skipped` loop (`:1408–1412`) and the `alternatives: none` push (`:1405`). `Sources`
   already printed those skips; SPEC CLI now says the confirmation repeats no skip.
6. **Tests** (in `mod tests`, all in-process):
   - `progress_writes_nothing_when_stderr_is_not_a_terminal`: drive `load/reference/probe×3/
     ranked/finish` on `Progress { enabled: false }` over a `RecordingSink`; assert
     `lines.is_empty() && clears == 0`.
   - `progress_names_each_phase_and_ends_with_the_summary`: the same sequence enabled; assert the
     exact `Vec<String>` (each transient line contains its phase name, probing lines carry
     `1/3`, `2/3`, `3/3`), that the last entry is the summary, and that no `rewrite` follows it.
   - `the_summary_line_has_one_decimal_place`: `done_line(4, 0, Duration::from_millis(3100)) ==
     "done — 4 usable, 0 rejected in 3.1 s"`, plus a 59 940 ms case (`"59.9 s"`).
   - `colour_needs_all_three_conditions`: the truth table over `colour_enabled` (terminal/not ×
     flag/no flag × `NO_COLOR` set/unset), with `assert_eq!`.
   - `a_plain_style_returns_the_input`: `Style::plain().bold("x") == "x"` for all six methods.
   - `a_coloured_report_differs_from_the_plain_one_only_by_escapes`: build the report twice,
     strip `\x1b\[[0-9;]*m` from the coloured lines, compare to the plain lines.
   - `the_list_report_reads_as_one_block` (`:3734`): **unchanged**, and it is the proof that a
     plain run's bytes did not move.
   - `a_report_with_no_rejection_prints_no_rejected_heading`: `rejected_lines(&[], false).is_empty()`.
   - `no_color_parses_and_has_no_short_form`: `--no-color` sets `Args::no_color`; `-N` stays an
     unknown option with exit 2.
7. **Run everything**, then open the PR. If the diff passes ~400 changed lines, split: colour and
   the three deletions first, progress second; say which half moved and why in the PR.

## Verification

```
CARGO_TARGET_DIR=/tmp/apt-scout-target cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/apt-scout-target cargo clippy --all-targets --locked -- -D warnings
CARGO_TARGET_DIR=/tmp/apt-scout-target cargo test --locked
CARGO_TARGET_DIR=/tmp/apt-scout-target cargo deny check
timeout 420 bash scripts/docker-test.sh > /tmp/d42-docker.log 2>&1   # then poll the log
rg -n 'keep as is|alternatives: none|Rejected — 0' src/ scripts/      # must print nothing
rg -n 'IsTerminal|var_os\("NO_COLOR"\)' src/                          # two hits, main.rs only
```

The matrix is the colour-off proof: every case captures the run through `$( )`, so stdout is a
pipe, colour is off, and `^Sources`, `Ranked`, `rewrite  line` and `skip` match today's bytes.

## Risks

- **Width formatting versus escapes** — the one real bug this can introduce. Paint completed
  cells only; the pinned whole-report test plus the strip-and-compare test catch a slip.
- **`n/m` timing** — `handle.join()` blocks in candidate order, so the count is a lower bound and
  a slow first candidate freezes `0/m`. Acceptable for a liveness line; do not reorder the joins.
- **Two stderr writers** — progress and warnings. Handled by `clear()` before the warning; a
  container case cannot see it, so it is a unit test on the seam or nothing.
- **A near-identical task exists** — D41 records the report's shape. This plan does not restate
  `sec:`/`redir:`/`~` rules, and the two must not both edit the same SPEC bullet.
- **Diff size** — eleven render sites plus four tests can pass 400 lines. Split rule in step 7.
