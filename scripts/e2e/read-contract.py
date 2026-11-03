"""Exercise positive filters and native Actions in a separate disposable repository."""
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request

TOKEN = os.environ['E2E_TOKEN']
REPO = 'dirtydishes/fga-read-probe'


def api(path, body=None, method=None):
    request = urllib.request.Request('http://forgejo:3000/api/v1' + path,
        data=None if body is None else json.dumps(body).encode(), method=method,
        headers={'Authorization': 'token ' + TOKEN, 'Content-Type': 'application/json'})
    with urllib.request.urlopen(request) as response:
        raw = response.read()
        return json.loads(raw) if raw else None


def gh(*args):
    result = subprocess.run(['gh', *args, '-R', 'forge.test/' + REPO], capture_output=True, text=True)
    assert result.returncode == 0, (args, result.stderr)
    return json.loads(result.stdout)


def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()


api('/user/repos', {'name': 'fga-read-probe', 'default_branch': 'main'})
Path('/tmp/read-probe').mkdir()
os.chdir('/tmp/read-probe')
git('init', '-q', '-b', 'main')
git('config', 'user.name', 'dirtydishes')
git('config', 'user.email', 'dirtydishes@users.noreply.invalid')
Path('.forgejo/workflows').mkdir(parents=True)
Path('.forgejo/workflows/ci.yaml').write_text('''name: native ci
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo probe
''')
# A second workflow, run only for the probe branch, so --workflow has runs to leave out.
Path('.forgejo/workflows/lint.yaml').write_text('''name: lint
on:
  push:
    branches: [probe]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - run: echo lint
''')
git('add', '.'); git('commit', '-qm', 'actions probe')
git('checkout', '-qb', 'probe')
Path('probe.txt').write_text('probe\n')
git('add', '.'); git('commit', '-qm', 'filter probe')
git('-c', 'http.extraHeader=Authorization: token ' + TOKEN, 'push', '-q',
    'http://forgejo:3000/' + REPO + '.git', '--all')
for attempt in range(40):
    if not api('/repos/' + REPO)['empty']: break
    time.sleep(0.25)
else: raise SystemExit('probe repository metadata did not settle')
label = api('/repos/' + REPO + '/labels', {'name': 'needs work', 'color': 'aa0000'})
api('/repos/' + REPO + '/pulls', {'base': 'main', 'head': 'probe', 'title': 'title match',
    'body': 'body only marker', 'labels': [label['id']], 'assignees': ['dirtydishes']})
api('/repos/' + REPO + '/issues', {'title': 'issue title match', 'body': 'body only marker',
    'labels': [label['id']], 'assignees': ['dirtydishes']})
assert gh('pr', 'list', '--label', 'needs work', '--author', '@me', '--assignee', '@me', '--head', 'probe', '--base', 'main', '--json', 'number') == [{'number': 1}]
assert gh('pr', 'list', '--label', 'missing', '--json', 'number') == []
assert gh('pr', 'list', '--label', 'needs work', '--label', 'missing', '--json', 'number') == []
assert gh('pr', 'list', '--search', 'marker in:title', '--json', 'number') == []
assert gh('issue', 'list', '--author', '@me', '--assignee', '@me', '--json', 'number') == [{'number': 2}]
assert gh('issue', 'list', '--label', 'needs work', '--json', 'number') == [{'number': 2}]
assert gh('issue', 'list', '--search', 'marker in:title', '--json', 'number') == []
# Unknown qualifiers must fail at the GraphQL boundary, with the GitHub error shape.
request = urllib.request.Request('https://forge.test/api/graphql',
    data=json.dumps({'query': '{ search(type:ISSUE,query:"repo:' + REPO + ' type:pr unsupported:value") { issueCount } }'}).encode(),
    headers={'Authorization': 'token ' + TOKEN, 'Content-Type': 'application/json'})
with urllib.request.urlopen(request) as response: error = json.load(response)['errors'][0]
assert error['type'] == 'UNPROCESSABLE' and 'unsupported:value' in error['message'] and error['path'] == ['search'], error
print('PASS positive author/assignee/quoted-label filters; unknown and missing labels; title scope; GitHub-shaped search errors', flush=True)

for attempt in range(60):
    runs = api('/repos/' + REPO + '/actions/runs')['workflow_runs']
    if len(runs) == 3: break
    time.sleep(0.5)
else: raise SystemExit('Forgejo did not create native Actions runs')
fields = 'status,conclusion,displayTitle,workflowName,headBranch,event'
actual = gh('run', 'list', '--limit', '5', '--json', fields)
key = lambda r: (r['workflowName'], r['headBranch'])
assert sorted(map(key, actual)) == [('lint', 'probe'), ('native ci', 'main'), ('native ci', 'probe')], actual
assert all(r['status'] == 'queued' and r['conclusion'] == '' and r['event'] == 'push' for r in actual), actual
assert gh('run', 'list', '--branch', 'probe', '--event', 'push', '--status', 'queued', '--json', fields) == [r for r in actual if r['headBranch'] == 'probe']
assert gh('run', 'list', '--status', 'completed', '--json', fields) == []
print('PASS native Forgejo Actions runs through real gh run list: queued, workflow name, branches, event', flush=True)
# --workflow by file name and by name: gh resolves the workflow, then reads
# actions/workflows/{id}/runs, which asks Forgejo for runs of that file.
for selector, name in (('ci.yaml', 'native ci'), ('native ci', 'native ci'), ('lint.yaml', 'lint')):
    by_workflow = gh('run', 'list', '--workflow', selector, '--json', fields)
    assert by_workflow == [r for r in actual if r['workflowName'] == name], (selector, by_workflow)
result = subprocess.run(['gh', 'run', 'list', '--workflow', 'missing.yml', '-R', 'forge.test/' + REPO], capture_output=True, text=True)
assert result.returncode != 0 and 'not found' in result.stderr, (result.returncode, result.stderr)
print('PASS real gh run list --workflow by file name and by name; a missing file is not found', flush=True)
# The simple JSON form needs only a mapped run, not the unsupported jobs/log APIs.
actual = gh('run', 'view', str(runs[0]['id']), '--json', 'status,headBranch,displayTitle')
assert actual['status'] == 'queued' and actual['headBranch'] in ('main', 'probe'), actual
print('PASS real gh run view with mapped JSON fields', flush=True)
# The default view follows the run's jobs_url and reads its artifacts; both answer empty lists,
# since Forgejo 15's API has neither per run.
result = subprocess.run(['gh', 'run', 'view', str(runs[0]['id']), '-R', 'forge.test/' + REPO], capture_output=True, text=True)
assert result.returncode == 0 and 'HTTP' not in result.stderr, (result.returncode, result.stdout, result.stderr)
print('PASS default run view shows a run with no jobs', flush=True)
