# Backlog history

Moved out of `ROADMAP.md` by D84: the evidence behind each task, kept verbatim.
The ledger holds the claim and the state; this file holds the gates, the verdicts and the logs.

### D76b

- [x] D76b. Give the requirements the normative voice: `MUST`/`SHOULD`/`MAY` per requirement, which means judging each of the 117 ids and rewording only where the current sentence does not already state an obligation. Done in this branch on `docs/normative-voice`: 117/117 ids carry at least one keyword on the line that names them; `SPEC.md`'s index was regenerated from those sentences and a verification loop proves every row occurs verbatim in its chapter; the counts hold at 32 planned, 6 partial, 79 implemented. `SHOULD` is used twice (`NFR-6` layout, `TUI-7` step count) because those are the two preferences between valid outcomes the text already stated. `NFR-5`'s milestone list — PR numbers and completion states — is deleted and the id now carries the rule that the sequence is `ROADMAP.md`'s. Two `docs/spec/AGENTS.md` P2s are closed: the lessons file is named as machine-level rather than at `.pi/agents/lessons.md`, and the id check is a set diff plus an occurrence count with what each proves. [SPEC: docs/spec/AGENTS.md]
