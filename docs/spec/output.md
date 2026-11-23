# Output

- `OUT-1` The default report MUST be the decision, and `--verbose` MUST be the proof.
  The default report answers
  "which mirror should I use and why" and `--verbose` answers "prove it to me", so a fact is in the
  default report only when a person choosing a mirror acts on it, and a fact that only lets a reader
  check the run is `--verbose`'s.
  Nothing below is optional and nothing below is a flag of its own: `-v` is the one switch, and it
  adds to the default report instead of replacing any part of it.
  Every one of these is `--verbose`'s alone, and the default report carries none of it: each row's
  AS number and its `same`/`diff`/`unknown` match with the server, and the chosen mirror's in the
  recommendation; the server's AS on the context line, which keeps its `mirrors:` and `reference:`
  fields; each row's country and the address the run resolved for it; both pocket dates in full,
  `-updates` and `-security`, and the `-security` lag; the exact signature status instead of the
  table's `yes`/`no`/`?`; a redirect host in full instead of the cut form; the full text of every
  rejection reason instead of the grouped class and its count.
  The default report keeps the identity line, `Sources` with its rewrite and skip reasons, the
  context line without the server's AS, the table, the recommendation, the planned change with the
  backup path, and the `Rejected` counts grouped by class. `--verbose` is display only in the same
  sense colour is: it reaches no decision, no ordering, no path, no byte written and no exit code,
  so two runs that differ only in it probe, rank, plan and write identically.
- `OUT-2` The ranked table's columns MUST be named for what they measure.
  `behind` is how far the mirror's
  `<codename>-updates` package lists trail the freshest date the run knows: minutes, `0m` when the
  mirror is up to date, and the same number `pipeline.md` step 8 rejects against, so it is freshness and never
  network latency. `latency` is the measured round trip of the mirror's `-updates` request, time to
  first byte as `pipeline.md` step 7 defines it, and never freshness. `verified` is the `<codename>-updates`
  `InRelease` signature: `yes` when `/usr/bin/gpgv` accepted it with the Ubuntu archive keyring,
  `no` when the body was readable but unsigned, altered or signed by an unknown key, and `?` when
  verification could not run at all — a missing `gpgv` or keyring, an oversized body, or a timeout.
  Only `yes` MAY authorize a write; `no` and `?` never do, and neither is a reason to reject or
  reorder a row, because read-only ranking keeps its tolerance (Freshness trust boundary).
  The legend printed with the `Ranked` title states one clause per column that needs one —
  `behind = -updates age vs freshest · latency = round trip · verified = InRelease signature` — and
  the recommendation uses these same words, never a second spelling of the same fact.
- `OUT-3` The trailing `notes` cell MUST carry exactly one fact, and a column that would be empty
  MUST be dropped.
  The cell is `redir:<host>` when the mirror redirects to another host, and the host is
  cut to at most 16 columns, ending in `~` when it did not fit; `~` cannot occur in a hostname, so a
  cut host is never read as a real one, and the marker costs the column it occupies so the cell
  stays inside its bound.
  Nothing else goes in that cell: the `-security` lag it once carried is proof about a pocket that
  decides nothing, and `Detail` prints that pocket's date and lag in full in a `--verbose` run.
  The AS column is omitted entirely when no shown row has AS data, and the `notes` column when no
  shown row has a note, so the table never reads as "every mirror is unknown" and a plain table
  gains no empty column.
- `OUT-4` A run MUST write its report once, on stdout, once probing and ranking are done: the identity
  line, `Sources`, the ranked section with its title, `Detail` in a `--verbose`
  run, `Recommendation`, `Planned change` when the mode plans one, in that order, and `Rejected`
  only when something was rejected.
  The identity line and `Sources` are always there; a section after them is printed when the run
  reaches it, so a run that stops before probing has neither.
  Nothing a run prints while it works can split the report.
  A section with nothing to say prints nothing at all — no heading, no `0` count — and every fact a
  section carries is the fact `CLI`, `Pipeline` 7–9 and `Apply` name, never a second copy of a fact
  another section has already printed.
  What a write prints after the confirmation (`backup:`, `wrote`, `changed`) is a second block,
  which this section does not change.
- `OUT-5` Colour MUST be display-only.
  Colour appears only when stdout is a terminal, `NO_COLOR` is not set
  (any value counts as set, the empty one included) and `--no-color` was not given; in every other
  case the report prints no escape sequence at all.
  Colour governs the report and nothing else: the progress line's own control sequences are not
  colour and are still written whenever stderr is a terminal, so a colour-off run on a terminal
  still redraws that one line and erases what it leaves with `ESC[K`.
  Where it is on: the identity line and every section heading are the accent, Ubuntu orange
  `#E95420`, **bold in the same escape** — `1;38;2;233;84;32` or `1;38;5;166` — and the Braille
  frame of the progress line is the same accent at plain weight, while the phase text beside it is
  not painted.
  The weight is part of the structure ink and not decoration: in the 256-colour form the accent's
  `166` sits close to the red a rejection reason and a `-` diff line carry, so two colours alone
  left structure and "this is wrong" looking alike on a dark terminal, and bold is most terminals'
  bright variant, which pulls the accent away from that dark-red end; the spinner keeps the plain
  weight because no red sits beside it and the accent was asked for there as it is.
  In the table `yes` is green and `no` and `?` are yellow; a rejection reason and a `-` diff line
  are red, a `+` diff line is green; the `behind`, `speed` and `latency` column headings are dim;
  the recommended mirror and the confirmation prompt are bold.
  Nothing else is styled, and the `sudo apt-scout …` line a non-root run prints is never styled,
  because a user copies it.
- `OUT-6` The accent MUST have two forms, `COLORTERM` MUST pick between them, and the fallback MUST be
  named because a terminal will not complain.
  `#E95420` is a 24-bit colour, so it is written as `38;2;233;84;32` — `1;38;2;233;84;32` where the
  site is bold — only when `COLORTERM` reads `truecolor` or `24bit`, in any case; every other value,
  and an unset `COLORTERM`, takes the 256-colour cube's `38;5;166` (`1;38;5;166` where the site is
  bold), which is the nearest orange a terminal without truecolor renders.
  A term that cannot render `38;2` ignores it or draws it wrong and says nothing, so a single
  unconditional truecolor escape would leave the accent invisible exactly where this fallback
  serves.
  Both forms and both weights come from one `Style` and are emitted by the one function that
  produces an escape, so the accent cannot be one form, or one weight, on one line and the other on
  the next: the form is decided once, with the colour decision, before the first line is printed.
- `OUT-7` The plain output MUST be the contract, and colour MUST reach no decision.
  With colour off the report
  carries the same words, the same columns and the same section headings a colour run prints, minus
  the escapes, so a harness that captures stdout matches the same lines either way: the container
  matrix runs with stdout a pipe, therefore colour off, and asserts that text.
  Colour is never an input to a decision, a comparison, an ordering, a path, the plan, the bytes
  written or an exit code, so two runs that differ only in colour write the same bytes under
  `/etc/apt` and exit with the same code.
- `OUT-8` Progress MUST be transient, animated, and only for a person.
  While a run works it shows one
  status line, on stderr, rewritten in place, and only when stderr is a terminal; when stderr is a
  pipe, a file or a CI log, no progress line is written at all — no timestamped line, no "still
  working" notice — and nothing is started to draw one.
  The line is **animated**: one Braille cell, advanced one frame about every 100 ms by a ticker for
  as long as a phase is active, so a phase that runs for five seconds is visibly moving instead of a
  still line that changes only when a count does.
  The phases it names are loading the mirror list, verifying the reference, probing, which carries
  `n/m`, the candidates probed so far, and ranking; the probing count renders inside the same frame
  as the cell, so one line carries both and neither is a second line.
  A warning keeps its own line on stderr: a progress rewrite never overwrites one, a warning never
  carries a progress line, and the animation is stopped while a warning is on screen and resumes
  with the next phase.
  The ticker advances a frame and redraws the line it is given and does nothing else: no probe's
  timeout, request, measurement, rejection or join order changes because a frame was drawn.
- `OUT-9` The animation MUST be testable without a terminal, and it MUST add no dependency.
  The frame is a pure
  function of the time a phase has been active, and the ticker's loop is driven through the existing
  progress `Sink` seam with an injected wait, so a synthetic 300 ms phase asserts the frames that
  were drawn and the text they carried, with no sleep, no thread and no terminal in the test.
  The animation is standard library only — `std::thread`, `std::sync::mpsc`, `std::sync::Mutex`,
  `std::time` — so no terminal crate, no progress crate and no new dependency is added, and a sink
  write that fails is dropped as before rather than turned into an error.
- `OUT-10` Progress MUST end with one summary line.
  The ticker is stopped and the live line cleared before
  the report is printed, and the ticker MUST NOT draw a frame after that: the status line is replaced by
  `done — <usable> usable, <rejected> rejected in <t> s`, with one decimal place, for example
  `done — 4 usable, 0 rejected in 3.1 s`.
  The counts are the ones the ranked title carries, and the time runs from the loading phase to the
  end of ranking; no transient line and no ticker outlives the report's first line.
- `OUT-11` `NO_COLOR`, `COLORTERM` and, later, the locale MUST be the only environment reads that may
  change printed text.
  `NO_COLOR` and `COLORTERM` are display-only reads — the first turns colour off,
  the second picks which of the accent's two escapes is printed — and the locale read of
  Localization is the other: none of the three reaches a decision, a path, a byte written or an exit
  code, and no module reached from `src/apply.rs` reads any of them.
  The accent adds no read beyond `COLORTERM`: `NO_COLOR`, `--no-color` and whether stdout is a
  terminal are the same three inputs as before and still decide whether any colour escape is
  printed, and `COLORTERM` is read once at startup beside them: with colour off it is read and it
  changes nothing, exactly as `NO_COLOR` is read on a run that has no `--no-color` to obey.

- `OUT-12` Text the run did not author MUST reach a terminal with every control character replaced, so a
  mirror cannot write an escape sequence into it.
  The text that is not the run's own is a mirror URL, its `Origin`, `Label` or error text, a rejected entry, and any
  error or warning that quotes one of them.
  A control character is written as the two characters `\\x` and two lowercase hex digits, so `ESC` prints as
  `\\x1b` and the reader can see what the mirror tried.
  The run's own escapes stay: the colour and the progress line are the run's, and this rule is about the bytes that
  come from outside it.
