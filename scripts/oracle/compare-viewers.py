"""Compare one gateway's read cases as an admin and as a non-admin user.

Every case must match exactly, except those whose answer depends on who is asking. A non-admin
reads branch protection through Forgejo's branch summary instead of the admin-only rules
(plan review F15), so any other difference is a gateway bug.

    compare-viewers.py ADMIN READER               # main unprotected
    compare-viewers.py --protected ADMIN READER   # main protected with required CI checks

With `main` protected, the summary hides the review and outdated-branch flags, so a non-admin's
merge state is UNKNOWN wherever an admin's is CLEAN or UNSTABLE; a known blocker stays BLOCKED for
both. That difference must appear, and PR 2's failing required check must block both.
"""
import difflib
import json
from pathlib import Path
import re
import sys

CASES = json.loads((Path(__file__).parent / 'cases.json').read_text())
# `@me` is the reader, who authored nothing: each case may differ only by being empty for them.
VIEWER_CASES = {
    'list-me-head': lambda admin: '[]\n',
    'issues-author': lambda admin: '[]\n',
    'status': lambda admin: re.sub(r'(?<=Created by you\n).*?(?=\n\nRequesting)', '  You have no open pull requests', admin, flags=re.S),
}


def partial(admin, reader):
    """The reader's output with each UNKNOWN merge state that stands for the admin's CLEAN or
    UNSTABLE put back, and how many it put back. Anything else stays as the reader saw it."""
    try: a, b = json.loads(admin), json.loads(reader)
    except ValueError: return reader, 0
    swapped = 0
    def walk(x, y):
        nonlocal swapped
        if isinstance(x, dict) and isinstance(y, dict):
            for key in y:
                if key == 'mergeStateStatus' and x.get(key) in ('CLEAN', 'UNSTABLE') and y[key] == 'UNKNOWN':
                    y[key] = x[key]
                    swapped += 1
                elif key in x:
                    walk(x[key], y[key])
        elif isinstance(x, list) and isinstance(y, list):
            for i, j in zip(x, y): walk(i, j)
    walk(a, b)
    return json.dumps(b) if swapped else reader, swapped


def merge_state(path):
    try: return json.loads(path.read_text()).get('mergeStateStatus')
    except (ValueError, AttributeError): return None


def main(admin, reader, protected=False):
    unexpected = swapped = 0
    for case in (c['id'] for c in CASES):
        diffs = []
        for ext in ('exit', 'stdout', 'stderr'):
            a = (admin / f'{case}.{ext}').read_text()
            b = (reader / f'{case}.{ext}').read_text()
            if ext == 'stdout' and case in VIEWER_CASES:
                if a == b:
                    diffs.append('no difference; remove it from VIEWER_CASES after review')
                    continue
                a = VIEWER_CASES[case](a)
            if ext == 'stdout' and protected and a != b:
                b, n = partial(a, b)
                if n: a, swapped = json.dumps(json.loads(a)), swapped + n
            diffs.extend(difflib.unified_diff(a.splitlines(), b.splitlines(), fromfile='admin.' + ext, tofile='reader.' + ext, lineterm=''))
        if diffs:
            unexpected += 1
            print(f'UNEXPECTED {case}')
            print('\n'.join(diffs))
        else:
            print(f'PASS {case}' + (' (empty for the reader)' if case in VIEWER_CASES else ''))
    checks = len(CASES)
    if protected:
        checks += 2
        blocked = [merge_state(root / 'probe-2.stdout') for root in (admin, reader)]
        if blocked != ['BLOCKED', 'BLOCKED']:
            unexpected += 1
            print(f'UNEXPECTED probe-2: a failing required check must block both viewers, got {blocked}')
        if not swapped:
            unexpected += 1
            print('UNEXPECTED no merge state was UNKNOWN for the reader: the branch summary went unread')
        else:
            print(f'PASS {swapped} merge states UNKNOWN for the reader where the admin sees CLEAN or UNSTABLE')
    print(f'Viewer summary: {checks - unexpected} expected, {unexpected} unexpected')
    return int(unexpected != 0)


if __name__ == '__main__':
    args = sys.argv[1:]
    protected = args[:1] == ['--protected']
    sys.exit(main(*map(Path, args[1:] if protected else args), protected=protected))
