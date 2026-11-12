# docs

## Purpose

Durable security and design decision records for apt-scout.

`threat-model.md` is the privilege-boundary record: what may cross into a privileged phase, that the program never obtains privilege for itself, the trusted root handoff, the S1–S16 security acceptance tests that gate D9, and the state of every `SPEC.md` delta those rulings require.

## Ownership

The architect writes and maintains these records. The human approves any change to a ruling or to the S1–S16 acceptance set; an agent may not soften or re-open one.

Task order, task states, merged PRs, and verification evidence live in `ROADMAP.md`, never here.

## Local Contracts

- One file per boundary decision; the privilege boundary is `threat-model.md`.
- This is a decision record, not a tutorial. It states what was decided, why, and what must be true before the next milestone. It does not restate `SPEC.md` and does not teach the codebase.
- It never adds a requirement that `SPEC.md` does not own. A requirement is written in the SPEC's voice and anchored by section; evidence about the current code is cited as `path:line` and is allowed to age.
- Superseded rulings are replaced in place, never annotated as history.
- Appendix A is the delta ledger: every `SPEC.md` change is either `APPLIED` with its section or `PENDING` with its owner. An item with neither is a defect.
- `plans/` stays untracked scratch for the coder and the reviewer; nothing durable lives there.

## Work Guidance

- Before changing a ruling, read `SPEC.md` "Privilege boundary and threat model", "Apply", "CLI", and "Pipeline".
- One change: the entry in the decision record, the body text that depended on it, and Appendix A. Then apply the SPEC delta or name its new owner.
- A second privileged entry point, a plan path, a setuid bit, and an internal `sudo` are ruled out; do not reintroduce them as an "option".
- English only.

## Verification

- Every claim about a control names a `SPEC.md` section or an S1–S16 test ID.
- `rg -n "^[0-9]+\. \*\*(APPLIED|PENDING)" docs/threat-model.md` lists the ledger; review it for an item that carries no state.
- `rg -n "^## " docs/threat-model.md` lists the record's sections; a ruling in the decision record that gates a write must point at least one S-ID.

## Child DOX Index

None. This folder has no subfolders.
