"""Compare one gateway's read cases as an admin and as a non-admin collaborator.

Every case must match exactly, except those whose answer depends on who is asking. A non-admin
reads branch protection through Forgejo's branch summary instead of the admin-only rules
(plan review F15), so any other difference is a gateway bug.
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


def main(admin, reader):
    unexpected = 0
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
            diffs.extend(difflib.unified_diff(a.splitlines(), b.splitlines(), fromfile='admin.' + ext, tofile='reader.' + ext, lineterm=''))
        if diffs:
            unexpected += 1
            print(f'UNEXPECTED {case}')
            print('\n'.join(diffs))
        else:
            print(f'PASS {case}' + (' (empty for the reader)' if case in VIEWER_CASES else ''))
    print(f'Viewer summary: {len(CASES) - unexpected} expected, {unexpected} unexpected')
    return int(unexpected != 0)


if __name__ == '__main__':
    sys.exit(main(*map(Path, sys.argv[1:])))
