#!/usr/bin/env python3
"""Documentation checks that used to be run by hand before every docs pull request.

Five checks, each one a defect that reached a merged pull request:

1. every `SPEC.md` row is five pipes wide (four cells: id, file, requirement, state), every
   cell is padded with exactly one space per side, the id and file cells are code spans, and
   the Requirement cell occurs in its chapter with whitespace collapsed on both sides;
   backticks are kept, so a cell that lost a code span fails;
2. no line in `SPEC.md`, `docs/spec/` or `docs/decisions/` is longer than 120 characters or 120
   bytes, outside tables and fenced blocks, with a line that is one code span exempt because a
   quoted message cannot be wrapped;
3. parentheses balance in every changed Markdown file, outside fenced blocks;
4. every relative link resolves from the directory of the file that contains it, outside fenced
   blocks, with percent-encoding decoded;
5. `docs/history/` is append-only against a base revision and against the working tree, so an
   uncommitted deletion is caught too.

Usage: python3 scripts/check-docs.py [BASE]

A BASE revision (for example origin/develop) is what checks 3 and 5 compare against. Without one
they are skipped and the script says so. A check that cannot run is a failure, not a pass: a base
with no merge base (a shallow clone) fails rather than reporting nothing.
"""

import os
import re
import subprocess
import sys
from urllib.parse import unquote

NL = chr(10)
BT = chr(96)
FENCE = BT * 3


class CheckError(Exception):
    """A check that cannot run is a failure, not a pass."""


def read(path):
    with open(path, encoding='utf-8') as handle:
        return handle.read()


def git(*args):
    result = subprocess.run(['git', *args], capture_output=True, text=True)
    if result.returncode != 0:
        raise CheckError('git ' + ' '.join(args) + ' failed: ' + result.stderr.strip())
    return result.stdout


def prose_lines(path):
    """The lines of a Markdown file that are not inside a fenced block."""
    fenced = False
    kept = []
    for line in read(path).split(NL):
        if line.startswith(FENCE):
            fenced = not fenced
            continue
        if not fenced:
            kept.append(line)
    return kept


def markdown_files():
    return [line for line in git('ls-files', '*.md').split(NL) if line.strip()]


def spec_rows():
    """Every SPEC.md row's Requirement cell occurs in its chapter, wrapping ignored."""
    failures = []
    rows = 0
    for line in read('SPEC.md').split(NL):
        if not line.startswith('| ' + BT):
            continue
        # A row is | id | file | requirement | state | : five pipes, four cells.
        if line.rstrip().count('|') != 5:
            failures.append('row does not have five pipes (four cells): ' + line[:70])
            continue
        parts = line.rstrip().split('|')
        cells = parts[1:5]
        padded = [c for c in cells if c != ' ' + ' '.join(c.split()) + ' ']
        if padded:
            failures.append('row padding is not a single space per side: ' + padded[0][:40])
            continue
        if not all(c.strip().startswith(BT) and c.strip().endswith(BT) for c in cells[:2]):
            failures.append('row id or file cell is not a code span: ' + line[:70])
            continue
        raw = parts[3]
        path = parts[2].strip().strip(BT)
        cell = raw.strip()
        if not os.path.exists(path):
            failures.append('SPEC.md names a chapter that does not exist: ' + path)
            continue
        rows += 1
        flat = ' '.join(cell.split())
        if flat not in ' '.join(read(path).split()):
            failures.append('row not in its chapter: ' + path + ': ' + cell[:70])
    return rows, failures


def width(files):
    """No prose line over 120 characters or bytes; tables, fences and quoted messages exempt."""
    failures = []
    single_span = re.compile(r'^\s*' + BT + r'[^' + BT + r']*' + BT + r'\s*$')
    for path in files:
        fenced = False
        for number, line in enumerate(read(path).split(NL), 1):
            stripped = line.rstrip()
            if stripped.startswith(FENCE):
                fenced = not fenced
                continue
            if fenced or stripped.startswith('|') or single_span.match(stripped):
                continue
            if len(stripped) > 120 or len(stripped.encode()) > 120:
                failures.append(path + ':' + str(number) + ' is ' + str(len(stripped.encode())) + ' bytes')
    return failures


def balance(files):
    """Parentheses balance, so a rewritten list cannot lose half a clause."""
    failures = []
    for path in files:
        if not os.path.exists(path):
            continue
        net = 0
        for line in prose_lines(path):
            net += line.count('(') - line.count(')')
        if net:
            failures.append(path + ' does not balance: ' + str(net) + ' unmatched parenthesis')
    return failures


def links(files):
    """Relative links resolve from the directory of the file that contains them."""
    failures = []
    for path in files:
        for match in re.finditer(r'\]\(([^)]+)\)', NL.join(prose_lines(path))):
            # A Markdown link may carry a title: [text](path "title").
            raw_target = match.group(1).split(' "')[0]
            target = unquote(raw_target)
            if target.startswith(('http', '#', 'mailto')):
                continue
            resolved = os.path.normpath(os.path.join(os.path.dirname(path), target.split('#')[0]))
            if not os.path.exists(resolved):
                failures.append(path + ' links to nothing: ' + target)
    return failures


def changed(base):
    """Files this branch touches, committed or not, so the check also runs before a commit."""
    names = set()
    for revision in (base + '...HEAD', base):
        for line in git('diff', '--name-only', revision).split(NL):
            if line.strip():
                names.add(line.strip())
    return sorted(names)


def append_only(base):
    """docs/history/ may grow, never change."""
    if not git('merge-base', base, 'HEAD').strip():
        raise CheckError('no merge base with ' + base + ', so the append-only check cannot run; '
                         'a shallow clone needs git fetch --unshallow')
    failures = []
    # The branch against its base, and the working tree against HEAD, so an uncommitted edit is
    # caught as well as a committed one.
    for revision in (base + '...HEAD', 'HEAD'):
        for line in git('diff', '--numstat', revision, '--', 'docs/history/').split(NL):
            if not line.strip():
                continue
            _, deleted, path = line.split(chr(9))
            if deleted != '0':
                failures.append(path + ' is history and lost ' + deleted + ' lines (' + revision + ')')
    return failures


def run(base):
    files = markdown_files()
    scope = ['SPEC.md']
    scope += sorted(p for p in files if p.startswith('docs/spec/'))
    scope += sorted(p for p in files if p.startswith('docs/decisions/'))

    rows, failures = spec_rows()
    print('spec rows', rows)
    failures += width(scope)
    failures += links(files)
    if base:
        touched = changed(base)
        failures += balance([p for p in touched if p.endswith('.md')])
        failures += append_only(base)
        print('base', base, 'files', len(touched))
    else:
        print('no base revision given: parenthesis balance and append-only are skipped')
    for failure in failures:
        print('FAIL ' + failure)
    print('failures', len(failures))
    return 1 if failures else 0


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else ''
    try:
        return run(base)
    except CheckError as error:
        print('FAIL ' + str(error))
        print('failures 1')
        return 1


if __name__ == '__main__':
    sys.exit(main())