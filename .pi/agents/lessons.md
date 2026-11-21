# Lessons

Mistakes made in this repository's work — what happened, why, and the path that works. Written by whoever
made the mistake, in the same turn, while the context that produced it is still alive.

The `mistake-logger` Pi extension appends the raw fact of every failed tool call to `## Raw facts` at the
bottom of this file: tool name, arguments, error, and `analyzed: no`. An entry is closed when an analysis
below names the cause and the correct path.
Before a pull request, `rg -n '^- analyzed: no$' .pi/agents/lessons.md` must return nothing: the pattern is
anchored to a whole line because the extension writes that line and this paragraph is prose.

An entry is not a diary and not an apology. It states the failure mode, why the habit that produced it is
wrong, and the rule that replaces it. Rules that outlive this file belong in `AGENTS.md`.

## Analyzed

### 2026-11-21 · A tool schema was assumed, not read

- **What happened**: `AskUserQuestion` was called without `multiSelect` on every question and the call was
  rejected with a validation error after the whole question set had been written.
- **Why**: the declaration was read, but only far enough to see the option shape; required fields were
  inferred from the example rather than from the schema.
- **Correct path**: read the full declaration before the first call — `describeTool(name)` returns the exact
  schema, including which fields are required. A validation failure costs the entire payload, not one field.

### 2026-11-21 · Backticks inside a codemode template literal

- **What happened**: three generated scripts failed to parse — `unexpected end of string`, then
  `unexpected token in expression: '^'` — because the Python or Markdown being generated contained
  backticks, which closed the JavaScript template literal host.
- **Why**: template literals were used to carry text that includes the one character that terminates them.
- **Correct path**: never put a backtick inside a `\``` template literal. Build the file with `tools.write`
  and a JSON string, use `chr(96)` / `String.fromCharCode(96)` inside the generated program, or join an
  array of single-quoted lines. When the generated text is Markdown for this repository, `chr(96)` is the
  default, not the exception.

### 2026-11-21 · A generator read the file it had already overwritten

- **What happened**: the second run of a split script failed with `KeyError: 'CLI'`; the script read
  `SPEC.md` as its input, but `SPEC.md` had already been replaced by the generated map.
- **Why**: input and output were the same path, and the script was written to be re-runnable.
- **Correct path**: copy the source once to an immutable path (`/tmp/SPEC.old.md`) and make every generator
  read that copy. A one-shot script that overwrites its own input is not re-runnable, and pretending
  otherwise hides the dependency until the second run.

### 2026-11-21 · The checker was wrong, and its 19 failures were believed

- **What happened**: a content-fidelity audit reported 19 missing lines. The audit had compared against the
  literal string `"SPEC.md "` instead of the file's contents, so the 15 lines that had merely moved to the
  root file were reported as lost, alongside the 2 that really were dropped.
- **Why**: the audit was trusted because it was the only signal, and a plausible failure count was not
  questioned.
- **Correct path**: validate a checker against a case it must catch and a case it must pass before reading
  its verdict, and treat an unexpected number of failures as a bug in the checker first.

### 2026-11-21 · A verification command that checked its own contract file

- **What happened**: the index check written into `docs/spec/AGENTS.md` iterated over `docs/spec/*.md`,
  which includes `AGENTS.md` itself, and reported `unindexed: docs/spec/AGENTS.md` as a failure on the first
  run.
- **Why**: the glob was written from the folder, not from the rule — a contract file is not a chapter.
- **Correct path**: a verification command must exclude the file that carries it, and it must be run before
  it is written down, not after.
- **Recurred the same day**: the pre-pull-request check was written as `rg -n "analyzed: no" .pi/agents/lessons.md`, which matches the prose above that explains the marker, so the check could never pass. The rule was right and the command was wrong.
- **Correct path**: anchor a check to a line only the machine writes (`^- analyzed: no$`), not to a token the prose also explains, and run the check before writing it down.

### 2026-11-21 · sed used as an editor, silently

- **What happened**: `sed -i` was used to patch a script's file read. The pattern was single-quoted, the
  file used double quotes, and the command reported success while changing nothing; the next run failed on
  the unchanged line.
- **Why**: an in-place edit with no verification is indistinguishable from a successful one.
- **Correct path**: patch generated or repository files through a tool that fails loudly on a missing match
  (a Python replacement with an assertion), or grep for the new text immediately after any `sed -i`.

### 2026-11-21 · Long tool output was lost to context clipping

- **What happened**: three `bash` calls printed 5–14 KB, the harness indexed them instead of showing them,
  and the content had to be re-fetched in small pieces.
- **Why**: output was printed wholesale because it was convenient for the script.
- **Correct path**: keep a single printed result under about 3 KB. Print the answer, not the data: aggregate
  in the script, write bulk output to a file, and read it back in slices.

### 2026-11-21 · A tool was assumed to exist behind the codemode wrapper

- **What happened**: `tools.subagent({action: "list"})` failed with `tools.subagent does not exist`;
  `subagent`, like `AskUserQuestion`, is a top-level tool, not one reachable from inside a codemode script.
- **Why**: `ALL_TOOLS` and the codemode `tools` proxy were assumed to be the same set.
- **Correct path**: check before calling — `ALL_TOOLS` lists what the sandbox can reach and
  `describeTool(name)` gives the schema; a tool missing from `ALL_TOOLS` is called directly, never through
  `tools.<name>`.

### 2026-11-21 · A claim entered the plan before it was checked

- **What happened**: the restructure plan asserted that `D6c`, `D7`, `D8`, `D9` and `D28` were decision
  records to be extracted into ADRs. Reading `docs/threat-model.md` showed it is itself the `D6e` decision
  record, and that most `D*` ids are task labels whose substance is a ruling in that file.
- **Why**: the ids came from the user's framing and from prose, and the repository was read for structure
  before it was read for meaning.
- **Correct path**: before a decision that shapes deliverables, find where the existing authority for that
  subject lives and read it. A plan built on an unread document is a guess with a deadline.

## Raw facts

Appended by the `mistake-logger` extension. `analyzed: no` means nobody has said yet why it happened.
