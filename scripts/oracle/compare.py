"""Compare real gh output with narrow normalizers and strict expected differences."""
import difflib
import json
from pathlib import Path
import re
import sys

from scrub import emails

CASES = json.loads((Path(__file__).parent / 'cases.json').read_text())


def text(value, case):
    value = emails(value.encode()).decode()
    value = value.replace('https://github.com/', 'https://HOST/').replace('https://forge.test/', 'https://HOST/')
    value = re.sub(r'/pulls/(\d+)', r'/pull/\1', value)
    value = re.sub(r'\b\d{4}-\d\d-\d\dT[^\s"\t]+', 'TIMESTAMP', value)
    value = re.sub(r'\b(?:about |less than )?\d+ (?:seconds?|minutes?|hours?|days?|weeks?|months?|years?) ago\b', 'RELATIVE_TIME', value)
    value = re.sub(r'\b[0-9a-f]{40}\b', 'SHA', value)
    if case.startswith('checks-'):
        # Tab columns are name, bucket, elapsed, link, event. Keep names, buckets, event.
        rows = []
        for line in value.splitlines():
            cols = line.split('\t')
            if len(cols) >= 4:
                cols[2] = 'DURATION'; cols[3] = 'CHECK_LINK'
            rows.append('\t'.join(cols))
        value = '\n'.join(rows) + ('\n' if value.endswith('\n') else '')
    return value


def normalize(value, case):
    try: data = json.loads(value)
    except ValueError: return text(value, case)
    def walk(obj, key=''):
        if isinstance(obj, dict): return {k: walk(v, k) for k,v in obj.items()}
        if isinstance(obj, list): return [walk(v, key) for v in obj]
        if isinstance(obj, str):
            if key in ('id', 'node_id'): return 'NODE_ID'
            if key in ('detailsUrl', 'targetUrl'): return 'CHECK_LINK'
            return text(obj, case)
        return obj
    return json.dumps(walk(data), sort_keys=True, indent=2) + '\n'


def known_difference(case, output):
    # Forgejo folds draft into mergeable=false. Only these two fields may differ.
    # Accept these exact values only, never arbitrary mismatches.
    if case in ('probe-4',):
        try: data = json.loads(output)
        except ValueError: return output
        if not isinstance(data, dict): return output
        if data.get('mergeStateStatus') == 'UNKNOWN': data['mergeStateStatus'] = 'CLEAN'
        if data.get('mergeable') == 'UNKNOWN': data['mergeable'] = 'MERGEABLE'
        return json.dumps(data, sort_keys=True, indent=2) + '\n'
    return output


EXPECTED_FAILURES = {
    'probe-4': 'Forgejo draft mergeability is ambiguous: mergeable and mergeStateStatus are UNKNOWN instead of MERGEABLE/CLEAN',
    'runs': 'The oracle twin has completed Actions runs; the status-only local seed has no workflow runs. Native Actions are checked separately.',
    'list-search': 'GitHub and Forgejo rank title-search matches differently; the complete result records must match as a multiset.',
    'issues-search': 'GitHub and Forgejo rank title-search matches differently; the complete result records must match as a multiset.',
}


def expected_difference(case, oracle, gateway):
    if case == 'probe-4': return oracle == known_difference(case, gateway)
    if case == 'runs':
        rows = [line.split('\t') for line in oracle.splitlines()]
        return gateway == '' and len(rows) == 5 and all(len(row) == 9 and row[:2] == ['completed', 'success'] and row[3] == 'ci' for row in rows)
    if case in ('list-search', 'issues-search'):
        try: a, b = json.loads(oracle), json.loads(gateway)
        except ValueError: return False
        keys = {'number'} if case == 'list-search' else {'number', 'state', 'title'}
        if not isinstance(a, list) or not isinstance(b, list) or not a or not b: return False
        if not all(isinstance(v, dict) and set(v) == keys for v in a + b): return False
        return sorted(json.dumps(v, sort_keys=True) for v in a) == sorted(json.dumps(v, sort_keys=True) for v in b)
    return False


# Any host, so a link to the wrong host is caught too (the normalizer folds hosts together).
PR_URL = re.compile(r'https?://([^/\s"]+)/dirtydishes/gfj-oracle/(pulls?)/(\d+)')
INVARIANT_FAILURES = {
    'pr-urls': 'MAPPING §8 decides /pull/N, but bead 65s has not built it, so the gateway still emits /pulls/N.',
}
# The only problems an expected failure covers: a forge.test link in Forgejo's /pulls/N layout.
INVARIANT_KNOWN = {'pr-urls': lambda problem: problem.startswith('layout ')}


def classify(matches, known, expected_failure):
    """pass, xfail or unexpected. A fixed expected failure is unexpected, so its entry gets removed."""
    if matches: return 'unexpected' if expected_failure else 'pass'
    return 'xfail' if known and expected_failure else 'unexpected'


def invariants(outputs, backend):
    """Check raw values a client uses where the normalizers above replace them: ChatGPT drops PRs
    whose URL isn't /pull/N, and passes headRefOid to `gh pr merge --match-head-commit`.
    `outputs` maps case IDs to gateway stdout; `backend` is Forgejo's own PR list."""
    heads = {pr['number']: pr['head']['sha'] for pr in backend}
    problems = {'head-oid': [], 'pr-urls': []}
    checked = urls = 0
    for case, stdout in outputs.items():
        for match in PR_URL.finditer(stdout):
            urls += 1
            if match[1] != 'forge.test': problems['pr-urls'].append(f'host {case}: {match[0]}')
            elif match[2] != 'pull': problems['pr-urls'].append(f'layout {case}: {match[0]}')
        try: data = json.loads(stdout)
        except ValueError: continue
        for pr in data if isinstance(data, list) else [data]:
            if not isinstance(pr, dict) or 'headRefOid' not in pr: continue
            checked += 1
            head = heads.get(pr.get('number'))
            if pr['headRefOid'] != head: problems['head-oid'].append(f"{case}: headRefOid {pr['headRefOid']}, Forgejo head {head}")
            commits = [c.get('oid') for c in pr.get('commits') or []]
            if commits and commits[-1] != head: problems['head-oid'].append(f'{case}: last commit {commits[-1]}, Forgejo head {head}')
    if not checked: problems['head-oid'].append('no case printed a headRefOid')
    if not urls: problems['pr-urls'].append('no case printed a PR URL')
    return problems


def main(expected, actual, backend):
    counts = dict(passed=0, xfail=0, unexpected=0)
    report = []
    for case in CASES:
        case = case['id']
        diffs = []
        matches = True
        known = True
        for ext in ('exit', 'stdout', 'stderr'):
            a = normalize((expected / f'{case}.{ext}').read_text(), case)
            b = normalize((actual / f'{case}.{ext}').read_text(), case)
            if a != b:
                matches = False
                known &= ext == 'stdout' and expected_difference(case, a, b)
                diffs.extend(difflib.unified_diff(a.splitlines(), b.splitlines(), fromfile='oracle.'+ext, tofile='gateway.'+ext, lineterm=''))
        expected_failure = case in EXPECTED_FAILURES
        state = classify(matches, known, expected_failure)
        counts[{'pass':'passed','xfail':'xfail','unexpected':'unexpected'}[state]] += 1
        reason = EXPECTED_FAILURES[case] if state == 'xfail' else ''
        print(f'{state.upper()} {case}' + (': ' + reason if reason else ''))
        if state != 'pass': print('\n'.join(diffs) if diffs else 'expected difference disappeared; remove xfail after review')
        report.append(dict(id=case, result=state, reason=reason, diff=diffs))
    outputs = {case['id']: (actual / f"{case['id']}.stdout").read_text() for case in CASES}
    for name, problems in invariants(outputs, json.loads(backend.read_text())).items():
        expected_failure = name in INVARIANT_FAILURES
        known = all(INVARIANT_KNOWN.get(name, lambda _: False)(problem) for problem in problems)
        state = classify(not problems, known, expected_failure)
        counts[{'pass':'passed','xfail':'xfail','unexpected':'unexpected'}[state]] += 1
        reason = INVARIANT_FAILURES[name] if state == 'xfail' else ''
        print(f'{state.upper()} invariant {name}' + (': ' + reason if reason else ''))
        if state == 'unexpected': print('\n'.join(problems) or 'expected failure disappeared; remove it after review')
        report.append(dict(id='invariant-' + name, result=state, reason=reason, diff=problems))
    (actual / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"Oracle summary: {counts['passed']} pass, {counts['xfail']} xfail, {counts['unexpected']} unexpected")
    return int(counts['unexpected'] != 0)


if __name__ == '__main__':
    sys.exit(main(*map(Path, sys.argv[1:])))
