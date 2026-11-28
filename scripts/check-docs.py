#!/usr/bin/env python3
"""Documentation checks that used to be run by hand before every docs pull request.

Five checks, each one a defect that reached a merged pull request:

1. every row of the one table `SPEC.md` may hold, walked by position from the header to the blank
   line that ends it: it starts with a pipe, is five pipes wide (four cells: id, file, requirement,
   state), every cell is one space of padding per side, the id and file cells are code spans, and
   the Requirement cell occurs in its chapter with whitespace collapsed on both sides. A row above
   the header or below the blank line, a second header, a header without its --- separator, or an
   empty table is a failure; fenced blocks are skipped;
2. no line in `SPEC.md`, `docs/spec/` or `docs/decisions/` is longer than 120 characters or 120
   bytes, outside tables and fenced blocks, with a line that is one code span exempt because a
   quoted message cannot be wrapped;
3. parentheses balance in every changed Markdown file, outside fenced blocks;
4. every relative link resolves from the directory of the file that contains it, outside fenced
   blocks, with percent-encoding decoded;
5. `docs/history/` only grows at its end: against the merge base, the text a file had must still
   be a prefix of it now, so a bullet inserted into a frozen entry fails as surely as a
   deletion, and a file the branch adds is allowed.

Usage: python3 scripts/check-docs.py [BASE]

A BASE revision (for example origin/develop) is what the parenthesis and history checks compare
against; without one they are skipped and the script says so. A check that cannot run is a
failure, not a pass: a base with no merge base (a shallow clone) fails rather than reporting
nothing.
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
    """The one table SPEC.md is allowed to hold: its rows, their shape, and what they state.

    SPEC.md holds exactly one table, and the walk covers the whole file: a row above the header or
    below the blank line that ends the table is a failure, and a fenced block is skipped the way the
    other checks skip it.
    """
    lines = read('SPEC.md').split(NL)
    fenced = set()
    inside = False
    for number, line in enumerate(lines):
        if line.startswith(BT * 3):
            inside = not inside
            fenced.add(number)
            continue
        if inside:
            fenced.add(number)
    headers = [i for i, line in enumerate(lines) if line.startswith('| ID |') and i not in fenced]
    if len(headers) != 1:
        return 0, ['SPEC.md must hold exactly one | ID | header outside a fenced block, found ' + str(len(headers))]
    header = headers[0]
    if header + 1 >= len(lines) or not re.match(r'^\|\s*-{3}', lines[header + 1]):
        return 0, ['the | ID | header is not followed by a --- separator row']
    failures = []
    rows = 0
    body = header + 2
    table = set(range(header, body))
    while body < len(lines) and lines[body].strip():
        table.add(body)
        line = lines[body]
        body += 1
        if not line.startswith('|'):
            failures.append('table row does not start with a pipe: ' + line[:70])
            continue
        if line.rstrip().count('|') != 5:
            failures.append('table row does not have five pipes (four cells): ' + line[:70])
            continue
        parts = line.rstrip().split('|')
        cells = parts[1:5]
        padded = [c for c in cells if c != ' ' + ' '.join(c.split()) + ' ']
        if padded:
            failures.append('row cell is not one space of padding per side: ' + padded[0][:40])
            continue
        if not all(c.strip().startswith(BT) and c.strip().endswith(BT) for c in cells[:2]):
            failures.append('row id or file cell is not a code span: ' + line[:70])
            continue
        path = cells[1].strip().strip(BT)
        cell = cells[2].strip()
        if not os.path.exists(path):
            failures.append('SPEC.md names a chapter that does not exist: ' + path)
            continue
        rows += 1
        if cell not in ' '.join(read(path).split()):
            failures.append('row not in its chapter: ' + path + ': ' + cell[:70])
    if not rows:
        failures.append('the SPEC.md table holds no rows')
    for number, line in enumerate(lines):
        if number in table or number in fenced:
            continue
        if re.match(r'^\s*\|', line):
            failures.append('table row outside the table: SPEC.md:' + str(number + 1))
    return rows, failures


def width(files):
    """No prose line over 120 characters or bytes; tables, fences and quoted messages exempt."""
    failures = []
    single_span = re.compile(r'^\s*' + BT + r'[^' + BT + r']*' + BT + r'\s*$')
    for path in files:
        if not os.path.exists(path):
            failures.append(path + ' is tracked but missing')
            continue
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
        if not os.path.exists(path):
            failures.append(path + ' is tracked but missing')
            continue
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
    """docs/history/ may grow at its end, and nowhere else.

    The comparison is against the merge base, so a develop that moved under the branch cannot make an
    innocent pull request fail, and a file the branch adds is allowed: a new history entry is an
    append. A file counted by lines would let a bullet be inserted into a frozen entry, which is the
    defect this rule exists for, so the test is a prefix test on the text.
    """
    merge_base = git('merge-base', base, 'HEAD').strip()
    if not merge_base:
        raise CheckError('no merge base with ' + base + ', so the append-only check cannot run; '
                         'a shallow clone needs git fetch --unshallow')
    failures = []
    touched = set()
    for revision in (merge_base, 'HEAD'):
        for line in git('diff', '--name-only', revision, '--', 'docs/history/').split(NL):
            if line.strip():
                touched.add(line.strip())
    for path in sorted(touched):
        if not os.path.exists(path):
            failures.append(path + ' is history and was deleted')
            continue
        known = subprocess.run(['git', 'cat-file', '-e', merge_base + ':' + path])
        if known.returncode != 0:
            continue  # a file the branch adds is an append by definition
        base_text = git('show', merge_base + ':' + path)
        if not read(path).startswith(base_text):
            failures.append(path + ' is history and an edit was written above its end')
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
