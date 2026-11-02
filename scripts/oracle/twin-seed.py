"""Seed the Forgejo twin dirtydishes/gfj-oracle with the GitHub twin's PR states (bead cwe.32, D5).

    TWIN_TOKEN=<dirtydishes token> python3 scripts/oracle/twin-seed.py
    TWIN_TOKEN=<token> python3 scripts/oracle/twin-seed.py --rehearse http://127.0.0.1:3000

PRs and issue numbers can't be undone, so every write is fenced to this repository, its `s/*`
branches and the routes below, and the run stops as soon as Forgejo returns a number the plan
didn't expect. It is resumable: items that already exist must match the plan by number and title,
or the run refuses before writing anything. `--rehearse` runs the same code against a loopback
Forgejo, such as a disposable container holding a copy of the twin.

PRs 1-10 and issues 11-12 mirror the GitHub twin (fixtures/oracle/seed.json) and get their CI from
the twin's own workflow on its runner, as `scenarios/github.sh` does on GitHub. The rest exist only
here: a fix pushed on a failing commit (F4), and reviews by a non-admin user.
"""
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

LIVE = 'https://git.dirtydishes.dev'
REPO = 'dirtydishes/gfj-oracle'
REVIEWER = 'gfj-e2e'
FIXTURES = Path(__file__).resolve().parents[2] / 'fixtures/oracle'
SEED = {pr['number']: pr for pr in json.loads((FIXTURES / 'seed.json').read_text())}


def pr(number, branch, commits, after=None, title=None, body=None, reviews=()):
    """`commits` is a list of {path: content or None to delete}; `after` is 'merge' or 'close'."""
    seed = SEED.get(number, {})
    title = title or seed['title']
    return dict(kind='pr', number=number, branch=branch, commits=commits, after=after,
                title=('WIP: ' if seed.get('draft') else '') + title,
                body=body if body is not None else seed.get('body') or '', reviews=reviews)


def note(name):
    return [{f'notes/{name}.txt': name + '\n'}]


PLAN = [
    pr(1, 's/pass', [{'notes/pass.txt': 'pass\n'}]),
    pr(2, 's/fail', [{'ci/fail': 'scenario s/fail\n'}]),
    pr(3, 's/pending', [{'ci/slow': '2700\n'}]),
    pr(4, 's/draft', [{'notes/draft.txt': 'draft\n'}]),
    pr(5, 's/nochecks', [{'.github/workflows/ci.yml': None}]),
    pr(6, 's/merged', note('merged'), after='merge'),
    pr(7, 's/closed', note('closed'), after='close'),
    pr(8, 's/merge-me', note('merge-me'), after='merge'),
    pr(9, 's/nopr', note('nopr')),
    pr(10, 's/gw-flow', note('gw-flow'), after='merge'),
    *(dict(kind='issue', number=n, **{k: json.loads((FIXTURES / f'issue-{n}.seed.json').read_text())[k]
          for k in ('title', 'body', 'state')}) for n in (11, 12)),
    pr(13, 's/fixed', [{'ci/fail': 'the first commit fails\n'}, {'ci/fail': None}],
       title='scenario: fixed on top', body='Oracle scenario `s/fixed`: the head passes, the first commit fails.'),
    pr(14, 's/reviewed', note('reviewed'), title='scenario: approved with a comment',
       body='Oracle scenario `s/reviewed`.',
       reviews=[dict(event='APPROVED', body='Looks good.',
                     comments=[dict(path='notes/reviewed.txt', new_position=1, body='A review thread.')])]),
    pr(15, 's/changes', note('changes'), title='scenario: changes requested',
       body='Oracle scenario `s/changes`.', reviews=[dict(event='REQUEST_CHANGES', body='Please change this.')]),
]
BRANCHES = {item['branch'] for item in PLAN if item['kind'] == 'pr'}


class Twin:
    def __init__(self, base, token):
        self.base, self.token = base.rstrip('/'), token
        self.api_root = self.base + '/api/v1/'

    def api(self, method, route, body=None, sudo=None):
        """Every write is checked against the plan; reads stay inside the repository or the reviewer."""
        allowed = route.startswith(f'repos/{REPO}') or route == f'users/{REVIEWER}' if method == 'GET' else (
            (method, route) in {('POST', f'repos/{REPO}/pulls'), ('POST', f'repos/{REPO}/issues'), ('POST', 'admin/users')}
            or re.fullmatch(rf'repos/{REPO}/pulls/\d+/(merge|reviews)', route) and method == 'POST'
            or re.fullmatch(rf'repos/{REPO}/(pulls|issues)/\d+', route) and method == 'PATCH' and body == {'state': 'closed'})
        if not allowed or (sudo not in (None, REVIEWER)) or (route == 'admin/users' and body.get('username') != REVIEWER):
            raise SystemExit(f'refusing {method} {route}: outside the twin-seed fence')
        headers = {'Authorization': 'token ' + self.token, 'Content-Type': 'application/json'}
        if sudo: headers['Sudo'] = sudo
        request = urllib.request.Request(self.api_root + route, method=method, headers=headers,
                                         data=None if body is None else json.dumps(body).encode())
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                raw = response.read()
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            error.msg = f'{error.msg} for {method} {route}: {error.read()[:300]!r}'
            raise

    def exists(self, route):
        try:
            self.api('GET', route)
            return True
        except urllib.error.HTTPError as error:
            if error.code == 404: return False
            raise

    def git(self, *args, cwd):
        header = 'Authorization: Basic ' + base64.b64encode(f'dirtydishes:{self.token}'.encode()).decode()
        return subprocess.run(['git', '-c', f'http.extraHeader={header}', *args], cwd=cwd, check=True,
                              capture_output=True, text=True).stdout.strip()


def existing(twin):
    """Every PR and issue already in the twin, by number, with its title."""
    found = {}
    for kind in ('pulls', 'issues'):
        page = 1
        while True:
            query = f'state=all&limit=50&page={page}' + ('&type=issues' if kind == 'issues' else '')
            batch = twin.api('GET', f'repos/{REPO}/{kind}?{query}')
            for item in batch: found[item['number']] = item['title']
            if len(batch) < 50: break
            page += 1
    return found


def check_resumable(found):
    planned = {item['number']: item['title'] for item in PLAN}
    for number, title in found.items():
        if planned.get(number) != title:
            raise SystemExit(f'#{number} {title!r} is not in the plan; refusing to seed on top of it')
    missing = [n for n in planned if n not in found]
    if missing and any(n > missing[0] for n in found):
        raise SystemExit(f'#{missing[0]} is missing but later numbers exist; the numbering cannot be repaired')


def push_branch(twin, checkout, item):
    twin.git('switch', '-q', '-C', item['branch'], 'origin/main', cwd=checkout)
    for i, commit in enumerate(item['commits']):
        for path, content in commit.items():
            target = checkout / path
            if content is None:
                twin.git('rm', '-q', path, cwd=checkout)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content)
                twin.git('add', path, cwd=checkout)
        message = item['title'].removeprefix('WIP: ') + (f' ({i + 1}/{len(item["commits"])})' if len(item['commits']) > 1 else '')
        twin.git('commit', '-q', '-m', message, cwd=checkout)
    if item['branch'] not in BRANCHES: raise SystemExit('branch outside the plan')
    twin.git('push', '-q', 'origin', f'refs/heads/{item["branch"]}:refs/heads/{item["branch"]}', cwd=checkout)


def wait(condition, what, timeout=120):
    deadline = time.monotonic() + timeout
    while not condition():
        if time.monotonic() > deadline: raise SystemExit(f'timed out waiting for {what}')
        time.sleep(2)


def seed(twin, checkout):
    found = existing(twin)
    check_resumable(found)
    if not twin.exists(f'users/{REVIEWER}'):
        twin.api('POST', 'admin/users', dict(username=REVIEWER, email=f'{REVIEWER}@users.noreply.invalid',
                 password=base64.b64encode(os.urandom(24)).decode(), must_change_password=False,
                 send_notify=False, visibility='public'))
        print(f'created non-admin user {REVIEWER}')
    for item in PLAN:
        number = item['number']
        if number not in found:
            if item['kind'] == 'issue':
                created = twin.api('POST', f'repos/{REPO}/issues', dict(title=item['title'], body=item['body']))
            else:
                if not twin.exists(f'repos/{REPO}/branches/{item["branch"]}'):
                    push_branch(twin, checkout, item)
                created = twin.api('POST', f'repos/{REPO}/pulls', dict(base='main', head=item['branch'],
                                   title=item['title'], body=item['body']))
            if created['number'] != number:
                raise SystemExit(f'Forgejo created #{created["number"]} where the plan expected #{number}; stopping')
            print(f'created #{number} {item["title"]}')
        kind = 'issues' if item['kind'] == 'issue' else 'pulls'
        current = twin.api('GET', f'repos/{REPO}/{kind}/{number}')
        if item.get('after') == 'merge' and not current.get('merged'):
            wait(lambda: twin.api('GET', f'repos/{REPO}/pulls/{number}')['mergeable'], f'#{number} to become mergeable')
            twin.api('POST', f'repos/{REPO}/pulls/{number}/merge', dict(Do='merge', delete_branch_after_merge=False))
            print(f'merged #{number}')
        if (item.get('after') == 'close' or item.get('state') == 'closed') and current['state'] != 'closed':
            twin.api('PATCH', f'repos/{REPO}/{kind}/{number}', dict(state='closed'))
            print(f'closed #{number}')
        if item.get('reviews'):
            done = twin.api('GET', f'repos/{REPO}/pulls/{number}/reviews')
            if not any((r.get('user') or {}).get('login') == REVIEWER for r in done):
                for review in item['reviews']:
                    twin.api('POST', f'repos/{REPO}/pulls/{number}/reviews', review, sudo=REVIEWER)
                print(f'reviewed #{number} as {REVIEWER}')


def main(argv):
    base = LIVE
    if argv[:1] == ['--rehearse']:
        base = argv[1]
        if urllib.parse.urlsplit(base).hostname not in ('127.0.0.1', 'localhost'):
            raise SystemExit('--rehearse only targets a loopback Forgejo')
    elif argv:
        raise SystemExit(__doc__)
    token = os.environ.get('TWIN_TOKEN') or sys.exit('set TWIN_TOKEN')
    twin = Twin(base, token)
    with tempfile.TemporaryDirectory() as temp:
        checkout = Path(temp)
        twin.git('clone', '-q', f'{base}/{REPO}.git', str(checkout), cwd=temp)
        twin.git('config', 'user.name', 'dirtydishes', cwd=checkout)
        twin.git('config', 'user.email', 'dirtydishes@users.noreply.invalid', cwd=checkout)
        seed(twin, checkout)
    print(json.dumps(existing(twin), indent=2, sort_keys=True))


if __name__ == '__main__':
    main(sys.argv[1:])
