#!/usr/bin/env python3
"""Triage the path:line citations in a document against the code they name.

For each citation into `src/` or `scripts/`, compare the words of five
characters or more in the unit that holds it with those of the cited line and
the five lines either side. A citation whose unit shares no such word with the
code needs a reader: the number may have drifted, or the sentence may be
describing something else.

A unit is a paragraph (the lines between two blank lines) or a single table
row, because the style contract wraps a sentence across lines and a fragment
shares fewer words with the code than the sentence does.

This is a triage, not a verdict. A flagged citation can be right and an
unflagged one can be wrong. Citations into documents rather than code are not
checked, and a range is judged by its first line.

Usage: python3 scripts/citation-triage.py [path-to-document]
"""

import re
import sys
from pathlib import Path

CITE = re.compile(r"((?:src|scripts)/[A-Za-z0-9_./-]+\.(?:rs|sh)):([0-9]+)")
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]{4,}")
WINDOW = 5


def words(text):
    return set(WORD.findall(text))


def units(lines):
    """Paragraphs and single table rows, as (first_line_number, text)."""
    out = []
    block, start = [], 0
    for number, line in enumerate(lines, start=1):
        stripped = line.strip()
        if stripped.startswith("|"):
            if block:
                out.append((start, " ".join(block)))
                block = []
            out.append((number, stripped))
        elif stripped and not stripped.startswith("#"):
            if not block:
                start = number
            block.append(stripped)
        elif block:
            out.append((start, " ".join(block)))
            block = []
    if block:
        out.append((start, " ".join(block)))
    return out


def main(argv):
    path = Path(argv[1] if len(argv) > 1 else "docs/threat-model.md")
    lines = path.read_text(encoding="utf-8").splitlines()
    sources = {}
    flagged = []
    total = 0
    for number, text in units(lines):
        for target, start in CITE.findall(text):
            total += 1
            if target not in sources:
                sources[target] = Path(target).read_text(encoding="utf-8").splitlines()
            body = sources[target]
            begin = max(0, int(start) - 1 - WINDOW)
            end = min(len(body), int(start) + WINDOW)
            context = words(" ".join(body[begin:end]))
            if not (words(text) & context):
                flagged.append((number, target, int(start)))
    targets = {(target, start) for _, target, start in flagged}
    print(f"citations: {total}")
    print(f"flagged: {len(flagged)} over {len(targets)} distinct targets")
    for number, target, start in flagged:
        print(f"  {path.name}:{number} -> {target}:{start}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
