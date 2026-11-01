"""Compare write results using the read oracle normalizers plus per-run identities."""
import difflib
import json
from pathlib import Path
import re
import sys

from compare import normalize
from writes import CASES


def canonical(raw, case, identity):
    number, branch = identity['number'], identity['branch']
    raw = raw.replace(branch, 'g3/' + case['id'])
    raw = re.sub(r'g3/(base-target|conflict-base)(?:-r\d+)?', r'g3/\1', raw)
    raw = re.sub(r'(/pulls?/)' + str(number) + r'\b', r'\g<1>PR', raw)
    raw = raw.replace('/pulls/PR', '/pull/PR')
    raw = re.sub(r'#' + str(number) + r'\b', '#PR', raw)
    # Comment IDs differ across hosts. Preserve the fragment kind and every comment field.
    raw = re.sub(r'#issuecomment-\d+', '#issuecomment-ID', raw)
    try:
        data = json.loads(raw)
        if isinstance(data, dict) and data.get('number') == number: data['number'] = 'PR'
        raw = json.dumps(data)
    except ValueError:
        pass
    return normalize(raw, case['id'])


def expected_difference(case, ext, oracle, gateway):
    if case == 'conflict' and ext == 'stderr':
        return (oracle == 'X Pull request dirtydishes/gfj-oracle#PR is not mergeable: the merge commit cannot be cleanly created.\nTo have the pull request merged after all the requirements have been met, add the `--auto` flag.\n'
                and gateway == 'GraphQL: Pull Request has merge conflicts (mergePullRequest)\n')
    return False


def main(expected, actual):
    report = []
    for case in CASES:
        for command in case['commands']:
            name = command['id']; differences = []; known = True
            output_match = state_match = True
            for ext in ('exit', 'stdout', 'stderr', 'state.json'):
                a = canonical((expected / f'{name}.{ext}').read_text(), case, json.loads((expected / f'{name}.identity.json').read_text()))
                b = canonical((actual / f'{name}.{ext}').read_text(), case, json.loads((actual / f'{name}.identity.json').read_text()))
                if a != b:
                    if ext == 'state.json': state_match = False
                    else: output_match = False
                    known &= expected_difference(name, ext, a, b)
                    differences += list(difflib.unified_diff(a.splitlines(), b.splitlines(), fromfile='oracle.'+ext, tofile='gateway.'+ext, lineterm=''))
            expected_failure = name == 'conflict'
            state = 'pass' if not differences and not expected_failure else 'xfail' if differences and known and expected_failure else 'unexpected'
            report.append(dict(id=name, result=state, output_match=output_match, state_match=state_match, diff=differences))
            print(state.upper() + ' write ' + name)
            if state != 'pass': print('\n'.join(differences) or 'expected difference disappeared; remove xfail after review')
    (actual / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    counts = {state: sum(r['result'] == state for r in report) for state in ('pass', 'xfail', 'unexpected')}
    print(f"Write oracle summary: {counts['pass']} pass, {counts['xfail']} xfail, {counts['unexpected']} unexpected")
    return int(counts['unexpected'] != 0)


if __name__ == '__main__':
    sys.exit(main(*map(Path, sys.argv[1:])))
