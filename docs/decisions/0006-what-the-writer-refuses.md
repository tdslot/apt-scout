# What the writer refuses, and why the refusals are explicit

The writer is the one process that runs as root, so every rule below is chosen to fail closed and to be
checkable before the first byte is written. The rules themselves are `docs/spec/apply.md`; this record keeps
the reasoning that shaped them.

## The baseline must arrive with the plan

The writer holds the selected mirror's proof against a freshness baseline before it writes anything, and
refuses outright when no verified baseline was supplied. The reason is structural: the writer makes no network
request, so it cannot establish a baseline for itself. A baseline computed inside the writer would be a network
fetch inside the privileged path, which the threat model rules out; passing it in keeps the only network code
unprivileged.

## A second restore is refused by the disk, not by a ledger

A restore backs up every file it replaces or removes, exactly as an apply does, so the newest backup after a
restore is the restore's own. The file the restore removed is recorded there as a file to put back, with its
bytes and a copy beside it. What therefore refuses a second restore from that backup is the record-versus-disk
check: the manifest says replace, and the target is gone. No second authority, no ledger of "already restored",
and no state the tool has to keep between runs.

## The refusal list names hosts on purpose

The list (`security.ubuntu.com`, `ports.ubuntu.com`, `ppa.launchpad.net`, `ppa.launchpadcontent.net`, and
anything under `launchpad.net` or `launchpadcontent.net`) is explicit and is checked before any membership
test. Before it existed, a PPA was safe only because no mirror list happened to name a PPA host, which is not a
control: it is a coincidence that a hostile or mistaken list can end. Naming the hosts makes the refusal a
decision the code makes, not one the data makes for it.

## A path prefix is recognition, and its cost is accepted

Recognizing the Ubuntu tree by path prefix means a host listed at a path other than the one an on-disk URI
carries is reported as skipped rather than forced through. The cost is accepted in that direction because the
alternative — writing a URI the tool is not sure about — is the failure the refusal exists to prevent.
A listed entry whose path is `/` admits every path on that host: the list names no narrower tree, and the tool
cannot tell the trees apart. That is why the canonical archive and its `*.archive.ubuntu.com` names stay
recognized at any path, and it is the one place where a bare listing widens rather than narrows.

## Candidate hygiene follows the transport

Every candidate must be `http`/`https`, carry no userinfo, and not resolve to a private, loopback or link-local
address. The input is a list fetched over plain HTTP from a host that can be wrong or hostile, so the answer is
validated as untrusted data rather than as a URL the tool chose.

**Spec**: `docs/spec/apply.md`, `APPLY-1` (the baseline), `APPLY-3` (the refusal list and the path prefix),
  `RESTORE-1` (the second restore) and `APPLY-14` (candidate hygiene). **Aliases**: none.
