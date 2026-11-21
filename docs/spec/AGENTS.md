# spec

## Purpose

The normative chapters of `SPEC.md`: one file per area, each owning one or more requirement
prefixes.
Code, tests and reviews are held to this text.

`../../SPEC.md` is the map: the requirement index and the cross-cutting rules. `../decisions/`
records why a rule is shaped the way it is; `../threat-model.md` rules on the privilege boundary.

## Ownership

The architect writes and maintains these chapters.
A change to a requirement is a behavior change: the chapter is updated before the code, in the same
pull request as the tests.

## Local Contracts

- One requirement per bullet, in the voice of the contract: `MUST`, `SHOULD`, `MAY`.
  The obligation is in the first sentence and that sentence carries the keyword: `MUST` for a hard
  obligation a command can check, `SHOULD` where the text names the justified exception or the
  choice between two valid outcomes, `MAY` for behavior that is genuinely optional.
  The body carries the same uppercase keywords wherever it states a further obligation or
  permission; a plain-English `may` or `must` stays lowercase.
  A sentence about what the code does today is a claim, not a requirement; claims live in
  `ROADMAP.md` with their evidence.
- Every requirement carries a stable id from the file's prefix (see the index in `SPEC.md`).
  An id is assigned once and never renumbered; a withdrawn requirement keeps its id and is marked
  withdrawn.
  The index in `SPEC.md` lists every id with one line and its state, and the `Requirement` cell is
  that requirement's first sentence verbatim, so a row can be found in its chapter by search.
- A requirement the code does not implement yet carries `(planned)` after its id.
  A requirement that is only partly implemented keeps no marker: the requirement stands as written, and its
  index row says `partial:` with what is missing.
- Terms come from `../../GLOSSARY.md`.
  A term the glossary lacks is added there first, in the same pull request.
- One sentence per line inside a paragraph or list item; a sentence too long to read is wrapped at
  a safe space.
  No line outside a table or a code block exceeds 120 characters.
  Tables only for enumerations.
- No history, no pull-request numbers, no evidence and no milestone notes: why a rule exists is an
  ADR, cited as `ADR-0003` and never restated.
  The delivery sequence and its states are `ROADMAP.md`'s alone.
  The rationale and the dated measurements some requirement bodies still carry are D83's work, not a
  licence for new ones.
- English only.

## Work Guidance

- Change the chapter before the code, in the same pull request, with the tests that prove it.
- A requirement moves between chapters only when the index in `SPEC.md` moves with it, in the same
  edit.
- A citation names a file and a step, never a bare section name: `pipeline.md` step 4, not
  "Pipeline 4".
- A rule that outlives its incident is promoted from the agent's lessons file, which is machine-level
  and not in this repository, into this contract; an incident that keeps recurring earns a
  verification command here.
- Run a verification command before writing it down, and check it against a case it must catch.

## Verification

Every chapter has at least one index row in `SPEC.md`:

(The loop matches the path inside a row, so a chapter named only in prose does not pass. The other direction,
an index row naming a chapter that does not exist, is not covered by a command; check it when a row is added.)

```
for f in docs/spec/*.md; do
  case "$f" in *AGENTS.md) continue;; esac
  rg -q "\| \`$f\` \|" SPEC.md || echo "unindexed: $f"
done
```

No line outside a table or a code block exceeds 120 characters, except a line that is one inline
code span:

```
awk '!/^[|]/ && length($0)>120 && $0 !~ /^ *`[^`]*`$/ {print FILENAME": "FNR}' SPEC.md docs/spec/*.md
```

Requirement ids are unique, and the index in `SPEC.md` and the chapters name the same set.
The first command compares the two sorted, unique id sets, so an id on one side and not the other is
printed; it says nothing about how often an id appears. The second counts occurrences, so an id twice
in one chapter and once in the index passes the first command and fails here.

```
ids() { rg --no-filename -o '`((CLI|OUT|PIPE|APPLY|RESTORE|ERR|LOC|TEST|NFR|PLAT|TUI)-[0-9]+)`' "$@" | sort -u; }
diff <(ids SPEC.md) <(ids docs/spec --glob '!AGENTS.md')
```

```
rg --no-filename -o '((CLI|OUT|PIPE|APPLY|RESTORE|ERR|LOC|TEST|NFR|PLAT|TUI)-[0-9]+)' \
  SPEC.md docs/spec | sort | uniq -c | awk '$1 != 2'
```

Every id appears exactly twice: once in its chapter and once in the index.
Each index row's `Requirement` cell is its chapter's first sentence, verbatim:

```
for f in docs/spec/*.md; do
  case "$f" in *AGENTS.md) continue;; esac
  flat=$(tr '\n' ' ' < "$f" | tr -s ' ' | sed 's/`//g')
  rg --no-filename "^\| \`[A-Z]*-[0-9]*\` \| \`$f\` \| " SPEC.md |
    sed -E 's/^\| `[^`]+` \| `[^`]+` \| (.*) \| [a-z:].*$/\1/' |
    sed 's/`//g' |
    while IFS= read -r s; do
      case "$flat" in *"$s"*) ;; *) echo "untraceable: $f: $s";; esac
    done
done
```

A row whose statement was reworded in the chapter and not in `SPEC.md` prints `untraceable`.
Each requirement states its obligation with one of the three keywords on the line that names its id:

```
pre='(CLI|OUT|PIPE|APPLY|RESTORE|ERR|LOC|TEST|NFR|PLAT|TUI)'
ids() { rg --no-filename -o "\x60$pre-[0-9]+\x60" "$@"; }
kw() { rg --no-filename -o "\x60$pre-[0-9]+\x60[^\n]*\b(MUST|SHOULD|MAY)\b" "$@" | rg -o '`[A-Z]+-[0-9]+`'; }
diff <(ids docs/spec --glob '!AGENTS.md' | sort -u) <(kw docs/spec --glob '!AGENTS.md' | sort -u)
```

An id whose line carries no keyword is printed on the left side alone; the keyword need not be the
first word, so a sentence that states its obligation after a preamble still counts.

## Child DOX Index

None.
