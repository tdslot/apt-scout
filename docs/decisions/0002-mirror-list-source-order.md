# Source the mirror list from Launchpad first and mirrors.ubuntu.com second

The primary mirror list is the Launchpad archive-mirrors page (`https://launchpad.net/ubuntu/+archivemirrors`, HTML,
about 316 KB, roughly 560 mirrors in about 80 countries).
It is the only source that carries each mirror's country and per-mirror metadata, and the whole country filter and the
widened run depend on that.
`http://mirrors.ubuntu.com/<CC>.txt` (plain text, one URL per line, already ordered by geographic proximity) is the
fallback when Launchpad fails or yields too few candidates.

Two further sources are planned and not implemented: a disk cache at `~/.cache/apt-scout/mirrors.json` valid for 24
hours, used only when the network sources fail, and a snapshot compiled into the binary with a warning when it is older
than 90 days.
Both sit behind the same order rather than replacing it: a live source that answers is never overridden by a cached one.

**Considered options**: mirrors.ubuntu.com alone — rejected, it carries no country and no per-mirror metadata;
hard-coding a snapshot only — rejected, it ages invisibly.

**Spec**: `docs/spec/pipeline.md`. **Aliases**: D28 (cache).
