"""Run the fixed read-only gh cases on either host. Never write the environment."""
import json
import os
import re
from pathlib import Path
import subprocess
import sys

from scrub import emails

ROOT = Path(__file__).resolve().parent
CASES = json.loads((ROOT / 'cases.json').read_text())
REPO = 'dirtydishes/gfj-oracle'

READS = {
    ('pr', 'list'),
    ('pr', 'list', '--state', 'all', '--limit', '20'),
    ('pr', 'list', '--state', 'merged'),
    ('pr', 'list', '--author', 'dirtydishes', '--state', 'all'),
    ('pr', 'list', '--author', '@me', '--head', 's/pass', '--state', 'all', '--json', 'number,url,state,headRefName'),
    ('pr', 'list', '--base', 'main', '--json', 'number'),
    ('pr', 'list', '--search', 'scenario in:title', '--state', 'all', '--json', 'number'),
    ('pr', 'diff', '1'),
    ('pr', 'diff', '1', '--name-only'),
    ('repo', 'view'),
    ('repo', 'view', '--json', 'name,owner,defaultBranchRef,url,description,isPrivate'),
    ('issue', 'list', '--state', 'all'),
    ('issue', 'view', '11'),
    ('issue', 'view', '12', '--json', 'number,state,title'),
    ('run', 'list', '--limit', '5'),
    ('api', 'repos/dirtydishes/gfj-oracle/commits/{pr1_head}/pulls', '--jq', '.[].number'),
    ('pr', 'list', '--assignee', 'dirtydishes', '--state', 'all', '--json', 'number'),
    ('pr', 'list', '--label', 'bug', '--state', 'all', '--json', 'number'),
    ('issue', 'list', '--author', '@me', '--state', 'all', '--json', 'number,state,title'),
    ('issue', 'list', '--search', 'scenario in:title', '--state', 'all', '--json', 'number,state,title'),
    ('issue', 'list', '--label', 'bug', '--state', 'all', '--json', 'number'),
    ('issue', 'list', '--assignee', '@me', '--state', 'all', '--json', 'number'),
    ('pr', 'list', '--state', 'closed', '--json', 'number,state'),
    ('pr', 'list', '--head', 's/pass', '--json', 'number'),
    ('pr', 'list', '--limit', '1', '--json', 'number'),
    ('issue', 'list'),
    ('pr', 'list', '--search', 'scenario in:title sort:created-desc', '--state', 'all', '--json', 'number'),
    ('pr', 'list', '--author', 'dirtydishes', '--state', 'closed', '--json', 'number,state'),
}


def allowed(args):
    return tuple(args) in READS or args == ['pr', 'status'] or (
        len(args) in (3, 5)
        and args[:2] in (['pr', 'view'], ['pr', 'checks'])
        and args[2] in ('1', '2', '4', '5', '6', '7')
        and (len(args) == 3 or (args[1] == 'view' and args[3] == '--json'
             and all(field.isidentifier() for field in args[4].split(','))))
    )


def debug_requests(debug):
    """Keep request documents and paths, never debug headers or response bodies."""
    requests = []
    for match in re.finditer(r'^GraphQL query:\n(.*?)^GraphQL variables: ([^\n]*)$', debug, re.M | re.S):
        requests.append(dict(query=match[1].rstrip(), variables=json.loads(match[2])))
    decoder = json.JSONDecoder()
    for match in re.finditer(r'^\{', debug, re.M):
        try: payload, _ = decoder.raw_decode(debug[match.start():])
        except ValueError: continue
        if isinstance(payload, dict) and isinstance(payload.get('query'), str):
            requests.append(payload)
    for match in re.finditer(r'^> (GET|POST) (\S+) HTTP/', debug, re.M):
        requests.append(dict(method=match[1], path=match[2]))
    return requests


def run(host, destination):
    destination.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env.update(GH_HOST=host, GH_PROMPT_DISABLED='1', GH_PAGER='cat', NO_COLOR='1', CLICOLOR='0', TZ='UTC')
    env.pop('GH_DEBUG', None)
    for case in CASES:
        args = case['args']
        # Exact read commands and bounded PR selections. Never invoke a shell.
        if not allowed(args) or re.fullmatch(r'[a-z0-9-]+', case['id']) is None:
            raise SystemExit('command outside read-only allowlist')
        if args[0] == 'api':
            if host == 'github.com':
                seed = json.loads((destination / 'seed.json').read_text())
                sha = next(pr['head']['sha'] for pr in seed if pr['number'] == 1)
            else:
                sha = subprocess.check_output(['git', 'rev-parse', 's/pass'], text=True).strip()
            if re.fullmatch(r'[0-9a-f]{40}', sha) is None:
                raise SystemExit('oracle PR head is not a full SHA')
            command = ['gh', 'api', '--method', 'GET', args[1].replace('{pr1_head}', sha), *args[2:], '--hostname', host]
        elif args[:2] == ['repo', 'view']:
            command = ['gh', *args, f'{host}/{REPO}']
        else:
            command = ['gh', *args, '-R', f'{host}/{REPO}']
        result = subprocess.run(command, env=env, capture_output=True)
        prefix = destination / case['id']
        prefix.with_suffix('.stdout').write_bytes(emails(result.stdout))
        prefix.with_suffix('.stderr').write_bytes(emails(result.stderr))
        prefix.with_suffix('.exit').write_text(str(result.returncode) + '\n')
        if host == 'github.com':
            debug = subprocess.run(command, env=dict(env, GH_DEBUG='api'), capture_output=True)
            requests = debug_requests(debug.stderr.decode(errors='replace'))
            prefix.with_suffix('.requests.json').write_bytes(emails((json.dumps(requests, indent=2) + '\n').encode()))
        print(f"captured {case['id']}: exit {result.returncode}")
        if host == 'github.com' and result.returncode != case['exit']:
            raise SystemExit(f"oracle {case['id']} exited {result.returncode}, expected {case['exit']}")


if __name__ == '__main__':
    host, output = sys.argv[1:]
    run(host, Path(output))
