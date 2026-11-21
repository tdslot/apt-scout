# spec

## Purpose

The normative chapters of `SPEC.md`: one file per area, each owning one requirement prefix.
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
- Every requirement carries a stable id from the file's prefix (see the index in `SPEC.md`).
  An id is assigned once and never renumbered; a withdrawn requirement keeps its id and is marked
  withdrawn.
- A requirement the code does not implement yet carries `(planned)` after its id.
- Terms come from `../../GLOSSARY.md`.
  A term the glossary lacks is added there first, in the same pull request.
- One sentence per line inside a paragraph or list item; a sentence too long to read is wrapped at
  a safe space.
  No line outside a table or a code block exceeds 120 characters.
  Tables only for enumerations.
- No history, no pull-request numbers, no evidence and no rationale: why a rule exists is an ADR,
  cited as `ADR-0003` and never restated.
- English only.

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

Requirement ids are unique per prefix:

```
rg -o '(CLI|OUT|PIPE|APPLY|RESTORE|ERR|LOC|TEST|NFR|PLAT|TUI)-[0-9]+' docs/spec | sort | uniq -d
```

## Child DOX Index

None.
