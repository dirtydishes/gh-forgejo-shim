"""Seed only the disposable Forgejo container from the committed GitHub snapshot."""
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request
from merge_retry import merge_retry

BASE = 'http://forgejo:3000/api/v1'
TOKEN = os.environ['E2E_TOKEN']
REPO = 'dirtydishes/gfj-oracle'


def api(path, body=None, method=None):
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(BASE + path, data=data, method=method,
        headers={'Authorization': 'token ' + TOKEN, 'Content-Type': 'application/json'})
    with urllib.request.urlopen(request) as response:
        raw = response.read()
        return json.loads(raw) if raw else None


def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()


api('/user/settings', {'full_name': 'dirtydishes'}, 'PATCH')
with urllib.request.urlopen('http://forgejo:3000/swagger.v1.json') as response:
    Path('/artifacts/forgejo-swagger.json').write_bytes(response.read())
seed = json.loads(Path('/test/oracle/seed.json').read_text())
metadata = json.loads(Path('/test/oracle/repository.seed.json').read_text())
api('/user/repos', {'name': 'gfj-oracle', 'private': metadata['private'], 'description': metadata['description'], 'default_branch': 'main'})
Path('/tmp/g2-seed').mkdir()
os.chdir('/tmp/g2-seed')
git('init', '-q', '-b', 'main')
git('config', 'user.name', 'dirtydishes')
git('config', 'user.email', 'dirtydishes@users.noreply.invalid')
Path('README.md').write_text(Path('/test/oracle/readme.md').read_text())
git('add', '.'); git('commit', '-qm', 'main')
for pr in sorted(seed, key=lambda pr: pr['number']):
    git('checkout', '-qb', pr['head']['ref'], 'main')
    if pr['number'] == 1:
        subprocess.run(['git', 'apply', '-'], input=Path('/test/oracle/diff-1.stdout').read_bytes(), check=True)
    else:
        Path(pr['head']['ref'].replace('/', '-') + '.txt').write_text(pr['head']['ref'] + '\n')
    title = pr['title']
    git('add', '.'); git('commit', '-qm', title)
git('-c', 'http.extraHeader=Authorization: token ' + TOKEN, 'push', '-q',
    'http://forgejo:3000/' + REPO + '.git', '--all')
for attempt in range(30):
    if not api('/repos/' + REPO)['empty']: break
    time.sleep(1)
else: raise SystemExit('Forgejo repository metadata did not settle')
for pr in sorted(seed, key=lambda pr: pr['number']):
    number = pr['number']
    title = ('WIP: ' if pr['draft'] else '') + pr['title']
    local = api('/repos/' + REPO + '/pulls', {'base': 'main', 'head': pr['head']['ref'], 'title': title, 'body': pr['body'] or ''})
    assert local['number'] == number, (local['number'], number)
    # Keep the status history, including stale failures, to exercise latest-per-context.
    if number in (1, 2, 3, 4, 8, 9, 10):
        sha = git('rev-parse', pr['head']['ref'])
        for job in ('check', 'lint'):
            for state, description in [('failure', 'old failure'), ('pending', 'Waiting to run'), ('success', 'Successful in 2s')]:
                if state == 'success' and number == 2 and job == 'check': state = 'failure'
                api('/repos/' + REPO + '/statuses/' + sha, {'context': f'ci / {job} (pull_request)', 'state': state,
                    'description': description, 'target_url': f'/dirtydishes/gfj-oracle/actions/runs/{number}/jobs/{1 if job == "check" else 2}'})
    if pr['merged_at']:
        for attempt in range(30):
            if api('/repos/' + REPO + f'/pulls/{number}')['mergeable']: break
            time.sleep(1)
        merge_retry(lambda: api('/repos/' + REPO + f'/pulls/{number}/merge',
            {'Do': 'merge', 'delete_branch_after_merge': False}), number)
    elif pr['state'] == 'closed':
        api('/repos/' + REPO + f'/pulls/{number}', {'state': 'closed'}, 'PATCH')
for number in (11, 12):
    issue = json.loads(Path(f'/test/oracle/issue-{number}.seed.json').read_text())
    local = api('/repos/' + REPO + '/issues', {'title': issue['title'], 'body': issue['body'] or ''})
    assert local['number'] == number, (local['number'], number)
    if issue['state'] == 'closed':
        api('/repos/' + REPO + f'/issues/{number}', {'state': 'closed'}, 'PATCH')
# Wait for Forgejo's asynchronous conflict check before gh's single read.
for attempt in range(30):
    pulls = api('/repos/' + REPO + '/pulls?state=open')
    if all(pr['mergeable'] or pr['draft'] for pr in pulls): break
    time.sleep(1)
else: raise SystemExit('Forgejo mergeability did not settle')
Path('/artifacts/forgejo-pulls.json').write_text(json.dumps(api('/repos/' + REPO + '/pulls?state=all'), indent=2))
print('Seeded oracle PRs 1-10 and issues 11-12, including status-only PRs 3 and 9')

# Forgejo's mergeable boolean also encodes WIP. Preserve the live single-PR response.
Path('/artifacts/forgejo-draft.json').write_text(json.dumps(api('/repos/' + REPO + '/pulls/4'), indent=2))
for name, refs in [('ahead', 'main...s/pass'), ('behind', 's/pass...main')]:
    Path('/artifacts/compare-' + name + '.json').write_text(json.dumps(api('/repos/' + REPO + '/compare/' + refs), indent=2))

sha = git('rev-parse', 's/pass')
Path('/artifacts/forgejo-statuses.json').write_text(json.dumps(api('/repos/' + REPO + '/commits/' + sha + '/statuses'), indent=2))
