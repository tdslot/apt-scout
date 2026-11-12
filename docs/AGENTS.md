# docs

## Purpose

Durable security and design decision records for apt-scout.

`threat-model.md` is the privilege-boundary record: what may cross into a privileged phase, that the program never obtains privilege for itself, the trusted root handoff, the S1–S17 security acceptance tests that gate D9, the state of every `SPEC.md` delta those rulings require, and the design gaps that still need a human ruling before D9 can start.

## Ownership

The architect writes and maintains these records. The human approves any change to a ruling, to the S1–S17 acceptance set, or the closure of an open design gap; an agent may not soften or re-open one.

Task order, task states, merged PRs, and verification evidence live in `ROADMAP.md`, never here.

## Local Contracts

- One file per boundary decision; the privilege boundary is `threat-model.md`.
- This is a decision record, not a tutorial. It states what was decided, why, and what must be true before the next milestone. It does not restate `SPEC.md` and does not teach the codebase.
- It never adds a requirement that `SPEC.md` does not own. A requirement is written in the SPEC's voice and anchored by section; evidence about the current code is cited as `path:line` and is allowed to age.
- Superseded rulings are replaced in place, never annotated as history.
- Appendix A is the delta ledger: every `SPEC.md` change is either `APPLIED` with its section or `PENDING` with its owner. An item with neither is a defect.
- `plans/` is untracked scratch for the coder and the reviewer; nothing durable lives there. That is a working convention, not an enforced rule: `.gitignore` does not list it, so keeping it out of commits is the author's job.
- An entry under "Open design gaps" is a question, not a decision. It is not written as settled, no milestone may implement an answer to it, and only a human ruling closes it.

## Work Guidance

- Before changing a ruling, read `SPEC.md` "Privilege boundary and threat model", "Apply", "CLI", and "Pipeline".
- One change: the entry in the decision record, the body text that depended on it, and Appendix A. Then apply the SPEC delta or name its new owner.
- A second privileged entry point, a plan path, a setuid bit, and an internal `sudo` are ruled out; do not reintroduce them as an "option". A root-spawned second privileged binary is out for v1 as well (decision record 4.4): the size of the privileged code reopens the ruling, it does not authorize a second artifact (4.6).
- The record's status line claims only what is true: nine rulings decided, `PENDING` items not yet in `SPEC.md`, and D6e open while an open design gap exists or the independent security review has not returned PASS.
- English only.

## Verification

- Every claim about a control names a `SPEC.md` section or an S1–S17 test ID.
- A control that exists is cited `path:line`; a control that must still be built is written as `must`, in the voice of the S tests, and says plainly that it does not exist yet. A present-tense sentence with no citation is a claim about shipped code, and a claim the code cannot support is a defect.
- `rg -n "OPEN — needs a human ruling" docs/threat-model.md` lists the undecided gaps; a gap entry without that marker, or a ruling that reads as if it answered one, is a defect.
- `rg -n "^[0-9]+\. \*\*(APPLIED|PENDING)" docs/threat-model.md` lists the ledger; review it for an item that carries no state.
- `rg -n "^## " docs/threat-model.md` lists the record's sections; a ruling in the decision record that gates a write must point at least one S-ID.

## Child DOX Index

None. This folder has no subfolders.
