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
  A sentence about what the code does today is a claim, not a requirement; claims live in
  `ROADMAP.md` with their evidence.
  The chapters carry ids but not yet the normative voice; that is D76b.
- Every requirement carries a stable id from the file's prefix (see the index in `SPEC.md`).
  An id is assigned once and never renumbered; a withdrawn requirement keeps its id and is marked
  withdrawn.
  The index in `SPEC.md` lists every id with one line and its state.
- A requirement the code does not implement yet carries `(planned)` after its id. A requirement that is only
  partly implemented keeps no marker — the requirement stands as written — and its index row says
`partial:` with what is missing.
- Terms come from `../../GLOSSARY.md`.
  A term the glossary lacks is added there first, in the same pull request.
- One sentence per line inside a paragraph or list item; a sentence too long to read is wrapped at
  a safe space.
  No line outside a table or a code block exceeds 120 characters.
  Tables only for enumerations.
- No history, no pull-request numbers, no evidence and no rationale: why a rule exists is an ADR,
  cited as `ADR-0003` and never restated.
  The chapters are not there yet — `non-functional.md` still carries milestone history — and
  removing it belongs to D76b.
- English only.

## Work Guidance

- Change the chapter before the code, in the same pull request, with the tests that prove it.
- A requirement moves between chapters only when the index in `SPEC.md` moves with it, in the same
  edit.
- A citation names a file and a step, never a bare section name: `pipeline.md` step 4, not
  "Pipeline 4".
- A rule that outlives its incident is promoted from `.pi/agents/lessons.md` into this contract;
  an incident that keeps recurring earns a verification command here.
- Run a verification command before writing it down, and check it against a case it must catch.

## Verification

Every chapter is in the `SPEC.md` index, and every index row has a file:

```
for f in docs/spec/*.md; do
  case "$f" in *AGENTS.md) continue;; esac
  rg -q "$f" SPEC.md || echo "unindexed: $f"
done
```

No line outside a table or a code block exceeds 120 characters, except a line that is one inline
code span:

```
awk '!/^[|]/ && length($0)>120 && $0 !~ /^ *`[^`]*`$/ {print FILENAME": "FNR}' SPEC.md docs/spec/*.md
```

Requirement ids are unique, and the index in `SPEC.md` and the chapters carry the same set:

```
rg --no-filename -o '((CLI|OUT|PIPE|APPLY|RESTORE|ERR|LOC|TEST|NFR|PLAT|TUI)-[0-9]+)' \
  SPEC.md docs/spec | sort | uniq -c | awk '$1 != 2'
```

Every id must appear exactly twice: once in its chapter and once in the index. A count of one or three is a
defect, and so is an id in a chapter that no index row names.

## Child DOX Index

None.
