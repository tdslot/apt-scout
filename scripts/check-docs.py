#!/usr/bin/env python3
"""Documentation checks the review keeps having to do by hand.

Five checks, each one a defect that reached a merged pull request:

1. every `SPEC.md` row's Requirement cell occurs in its chapter character for character;
2. no line in `SPEC.md`, `docs/spec/` or `docs/decisions/` is longer than 120 characters or bytes,
   outside tables and fenced blocks, with a line that is one code span exempt (a quoted message);
3. parentheses balance in every Markdown file the diff touches;
4. every relative link resolves from the file that contains it;
5. `docs/history/` is append-only against a base revision, when one is given.

Usage: python3 scripts/check-docs.py [BASE]

A BASE revision (for example origin/develop) enables check 5 and scopes check 3 to changed files.
"""

import os
import re
import subprocess
import sys

NL = chr(10)
BT = chr(96)


def read(path):
    with open(path, encoding='utf-8') as handle:
        return handle.read()


def git(*args):
    result = subprocess.run(['git', *args], capture_output=True, text=True)
    return result.stdout if result.returncode == 0 else ''


def changed(base):
    """Files this branch touches, committed or not, so the check also runs before a commit."""
    names = set()
    for revision in (base + '...HEAD', base):
        for line in git('diff', '--name-only', revision).split(NL):
            if line.strip():
                names.add(line.strip())
    return sorted(names)


def spec_rows():
    """Every row whose Requirement cell must occur verbatim in its chapter."""
    failures = []
    rows = 0
    for line in read('SPEC.md').split(NL):
        if not line.startswith('| ' + BT):
            continue
        parts = line.rstrip().split('|')
        if len(parts) < 6:
            continue
        path = parts[2].strip().strip(BT)
        cell = parts[3].strip()
        if not os.path.exists(path):
            failures.append('SPEC.md names a chapter that does not exist: ' + path)
            continue
        rows += 1
        if ' '.join(cell.split()) not in ' '.join(read(path).split()):
            failures.append('row not verbatim in ' + path + ': ' + cell[:70])
    return rows, failures


def width(files):
    """No prose line over 120 characters or bytes; tables, fences and quoted messages exempt."""
    failures = []
    single_span = re.compile(r'^\s*' + BT + r'[^' + BT + r']*' + BT + r'\s*$')
    for path in files:
        fenced = False
        for number, line in enumerate(read(path).split(NL), 1):
            stripped = line.rstrip()
            if stripped.startswith(BT * 3):
                fenced = not fenced
                continue
            if fenced or stripped.startswith('|') or single_span.match(stripped):
                continue
            if len(stripped) > 120 or len(stripped.encode()) > 120:
                failures.append(path + ':' + str(number) + ' is ' + str(len(stripped.encode())) + ' bytes')
    return failures


def balance(paths):
    """Parentheses balance, so a rewritten list cannot lose half a clause."""
    failures = []
    for path in paths:
        if not os.path.exists(path):
            continue
        text = read(path)
        net = text.count('(') - text.count(')')
        if net:
            failures.append(path + ' does not balance: ' + str(net) + ' unmatched parenthesis')
    return failures


def links(files):
    """Relative links resolve from the directory of the file that contains them."""
    failures = []
    for path in files:
        for match in re.finditer(r'\]\(([^)]+)\)', read(path)):
            target = match.group(1)
            if target.startswith(('http', '#', 'mailto')):
                continue
            resolved = os.path.normpath(os.path.join(os.path.dirname(path), target.split('#')[0]))
            if not os.path.exists(resolved):
                failures.append(path + ' links to nothing: ' + target)
    return failures


def append_only(base):
    """docs/history/ may grow, never change."""
    failures = []
    out = git('diff', '--numstat', base + '...HEAD', '--', 'docs/history/')
    for line in out.split(NL):
        if not line.strip():
            continue
        added, deleted, path = line.split(chr(9))
        if deleted != '0':
            failures.append(path + ' is history and lost ' + deleted + ' lines')
    return failures


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else ''
    all_md = [line for line in git('ls-files', '*.md').split(NL) if line.strip()]
    spec_scope = ['SPEC.md']
    spec_scope += sorted(p for p in all_md if p.startswith('docs/spec/') and not p.endswith('AGENTS.md'))
    spec_scope += sorted(p for p in all_md if p.startswith('docs/decisions/') and p.endswith('.md'))

    rows, failures = spec_rows()
    print('spec rows', rows)
    failures += width(spec_scope)
    failures += links(all_md)
    if base:
        touched = changed(base)
        # AGENTS.md files carry shell loops, whose process substitution never balances.
        failures += balance([p for p in touched if p.endswith('.md') and os.path.basename(p) != 'AGENTS.md'])
        failures += append_only(base)
        print('base', base, 'files', len(touched))
    else:
        print('no base revision given: parenthesis balance and append-only are skipped')

    for failure in failures:
        print('FAIL ' + failure)
    print('failures', len(failures))
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())