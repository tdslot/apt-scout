# Why the exit codes are five, and why a date can rank but never install

Two decisions in `docs/spec/non-functional.md` need their why recorded elsewhere: what the exit codes mean to a
script, and how far a freshness date can reach.

## The exit codes are a small vocabulary, not a failure list

`0`, `1`, `2`, `3` and `4` are distinct because a caller branches on them, and the two that look like errors are not:

- `3` says the run would write and this uid may not. It is the expected outcome of running the tool without `sudo`,
  so folding it into `1` would make the normal path indistinguishable from a broken one, and a wrapper script could
  not tell "run me again privileged" from "this failed".
- `4` says there is nothing here to rank: an unsupported distribution or architecture, or a release that has left
  `archive.ubuntu.com`. That is a platform verdict rather than a runtime fault, so a caller can decide to give up
  instead of retrying. `1` is the failure a rerun does not fix: the input has to change, or the run has
  to be repaired.

A usage error is `2` and keeps that code wherever it is noticed, because the fix is the same: change the command.

## A forged or unverifiable date can mis-rank, and nothing more

Ranking may read an `InRelease` `Date` whose signature is `invalid` or `unavailable`; that is a deliberate trade,
because a rank is advice and a fresh mirror list is what the user came for. What it cannot do is authorize an
installation: `apt` still checks archive signatures for every package it installs, so a hostile date moves a mirror up
a table and stops there. `docs/threat-model.md` records that boundary as A5/A6.

The write path is kept apart from the ranking path by two types rather than by discipline. `VerifiedDate` wraps a
date the strict in-region parser produced, and `SignatureStatus::Verified` carries a `VerifiedProof` whose only field
is private and minted by one code path: `GOODSIG`, `VALIDSIG` and exit 0. A caller can therefore neither forge the
status nor pair a date it read with a proof it did not earn, and read-only ranking still works when verification is
unavailable while write mode fails closed.

**Spec**: `docs/spec/non-functional.md`, `NFR-4` (the codes) and `NFR-10` (the boundary). **Aliases**: D25.