#!/usr/bin/env python3
"""Triage the path:line citations in docs/threat-model.md against the code.

For every `path:line` citation, compare the identifiers on the citing line with
the identifiers in the five lines either side of the cited one. A citation whose
line shares no identifier with the code it names needs a reader: the number may
have drifted, or the sentence may be describing something else. This is a
triage, not a verdict -- a flagged citation can be right and an unflagged one
can be wrong.

Usage: python3 scripts/citation-triage.py [path-to-threat-model]
"""

import re
import sys
from pathlib import Path

CITE = re.compile(r"((?:src|scripts)/[A-Za-z0-9_./-]+\.(?:rs|sh)):([0-9]+)")
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]{4,}")
WINDOW = 5


def words(text):
    return set(WORD.findall(text))


def main(argv):
    path = Path(argv[1] if len(argv) > 1 else "docs/threat-model.md")
    lines = path.read_text(encoding="utf-8").splitlines()
    sources = {}
    flagged = []
    total = 0
    for number, line in enumerate(lines, start=1):
        for target, start in CITE.findall(line):
            total += 1
            if target not in sources:
                sources[target] = Path(target).read_text(encoding="utf-8").splitlines()
            body = sources[target]
            begin = max(0, int(start) - 1 - WINDOW)
            end = min(len(body), int(start) + WINDOW)
            context = words(" ".join(body[begin:end]))
            if not (words(line) & context):
                flagged.append((number, target, int(start)))
    targets = {(target, start) for _, target, start in flagged}
    print(f"citations: {total}")
    print(f"flagged: {len(flagged)} over {len(targets)} distinct targets")
    for number, target, start in flagged:
        print(f"  docs/threat-model.md:{number} -> {target}:{start}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))