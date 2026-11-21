# TUI

Library: `ratatui` + `crossterm` (no async).
Opens only with `--tui`, after probing finishes (a progress view is shown while probing).

- Layout: ranked mirror table (left/top), a detail panel for the highlighted mirror, and a footer
  with the active keys.
- Table columns: checkbox, rank, host, country, scheme, latency, throughput, lag (updates), AS
  match, signature, `Recommended` badge.
- Detail panel: full URL, country, AS number and name, resolved IP, scheme, time to first byte,
  throughput, `Date` and lag for `-updates` (decides) and `-security` (informational), AS match,
  signature status of the `-updates` `InRelease`, mirror list source, status or rejection reason.
- Recommendation: the best-ranked mirror is pre-checked and carries the `Recommended` badge.
  Rejected mirrors are shown dimmed, with the reason, and cannot be checked.
- Selection: multiple checkboxes.
  The first checked mirror in rank order is the primary (active URI); other checked mirrors are
  written as commented `alternative` lines (see Apply).
- Keys (arrows are primary; `j/k` not required):
  - `Up`/`Down`: move in the focused section. `Left`/`Right`: switch tab/page within a section
    (e.g. the `-updates` and `-security` detail tabs) or collapse/expand details.
  - `Tab`/`Shift+Tab`: cycle focus between sections (mirror table, detail panel, buttons).
  - `Space`: toggle checkbox on the highlighted mirror.
  - `Enter`: confirm the current step or dialog (accept, yes).
  - `Esc`: cancel the current action or close a dialog; on the main screen, quit without changes
    (asks to confirm if the selection was changed).
  - `Insert` (implemented in the last milestone): add an entry: opens an input dialog for a
    custom mirror URL (validated, then probed like any candidate and added to the table).
  - `Delete`/`Del` (last milestone): remove the highlighted entry from the list (asks to
    confirm; only affects the current run, never apt files).
  - `r`: re-probe. `?`: help.
- Zero-effort flow: defaults must be right, so pressing `Enter` through the flow saves the best
  fresh mirror among the existing candidates.
  Keep the step count minimal, but never hide information the user needs.
  Base path (3 steps):
  1.
     Mirror table with the `Recommended` mirror pre-checked and a one-line summary of what will
     change; `Enter` continues.
  2.
     Confirmation dialog `Apply changes? [Yes]`: lists the files to change, old URI -> new URI,
     alternatives, and the backup path; `Yes` focused.
     As root, `Enter` validates the plan and writes it, so the dialog is the confirmation and
     `--yes` is not needed.
     As a non-root uid, `Enter` prints the one line `sudo apt-scout --tui` and exits 3; the TUI
     writes nothing.
  3.
     Result screen: as root, `applied <mirror> to <files>`, the backup path the write created, how
     to undo (`--restore`), and the suggested `sudo apt update`; as a non-root uid,
     `nothing written, run sudo apt-scout --tui`, the files the write would change and the backup
     path it would create. `Enter` exits.
- Extra steps appear only when the user must be informed or decide: selection has warnings (e.g.
  only http, lag near `MAX_LAG`, different country), a custom mirror failed its probe, no fresh
  mirror found, the write failed (show the reason and the state of the backup, which is untouched).
- Every dialog shows its available keys in a footer bar.
- Applying from the TUI uses the same writer as the default run: as root, `Enter` on the
  confirmation validates the plan and writes it; as a non-root uid the TUI writes nothing and prints
  the `sudo apt-scout --tui` line.
  Every other Apply rule below holds unchanged.
- Terminal state is always restored on exit, panic or Ctrl-C.
