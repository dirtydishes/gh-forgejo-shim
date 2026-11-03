"""Seed the Forgejo twin dirtydishes/gfj-oracle with the GitHub twin's PR states (bead cwe.32, D5).

    TWIN_TOKEN=<dirtydishes token> python3 scripts/oracle/twin-seed.py
    TWIN_TOKEN=<token> python3 scripts/oracle/twin-seed.py --rehearse http://127.0.0.1:3000

PRs and issue numbers can't be undone, so every write is fenced to this repository, its `s/*`
branches and the routes below, and the run stops as soon as Forgejo returns a number the plan
didn't expect. It is resumable: items that already exist must match the plan by number, title and
head branch, or the run refuses before writing anything. `--rehearse` runs the same code against a
loopback Forgejo, such as a disposable container holding a copy of the twin.

PRs 1-10 and issues 11-12 mirror the GitHub twin (fixtures/oracle/seed.json) and get their CI from
the twin's own workflow on its runner, as `scenarios/github.sh` does on GitHub. The rest exist only
here: a fix pushed on a failing commit (F4), and reviews by a non-admin user. Those reviews aren't
official (gfj-e2e can't write), so they feed review lists and threads, not reviewDecision, which
the GitHub twin can't show either: it has no branch protection.

#3 shows its checks passing until the end of the run, when its 45-minute job starts; it is pending
only while that job waits and runs, so live captures of a pending PR must fall in that window.

The token never reaches argv or error output: git gets it through GIT_CONFIG_* variables, and the
user's own git config is ignored, so no insteadOf rule can reroute the push over SSH.
"""
import base64
import http.client
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
CHECK = 'ci / check (pull_request)'
FIXTURES = Path(__file__).resolve().parents[2] / 'fixtures/oracle'
SEED = {pr['number']: pr for pr in json.loads((FIXTURES / 'seed.json').read_text())}


def pr(number, branch, commits, after=None, title=None, body=None, reviews=(), then=None):
    """`commits` are {path: content, or None to delete}, pushed before the PR opens. `then` holds
    commits pushed after it: 'failed' ones once the PR's first run has failed, 'last' ones at the
    very end of the run. `after` is 'merge' or 'close'."""
    seed = SEED.get(number, {})
    title = title or seed['title']
    return dict(kind='pr', number=number, branch=branch, commits=commits, after=after,
                title=('WIP: ' if seed.get('draft') else '') + title,
                body=body if body is not None else seed.get('body') or '', reviews=reviews, then=then)


def note(name):
    return [{f'notes/{name}.txt': name + '\n'}]


PLAN = [
    pr(1, 's/pass', [{'notes/pass.txt': 'pass\n'}]),
    pr(2, 's/fail', [{'ci/fail': 'scenario s/fail\n'}]),
    # The 45-minute job comes last, so no other run ever queues behind it on the twin's runner.
    pr(3, 's/pending', note('pending'), then=('last', [{'ci/slow': '2700\n'}])),
    pr(4, 's/draft', [{'notes/draft.txt': 'draft\n'}]),
    pr(5, 's/nochecks', [{'.github/workflows/ci.yml': None}]),
    pr(6, 's/merged', note('merged'), after='merge'),
    pr(7, 's/closed', note('closed'), after='close'),
    pr(8, 's/merge-me', note('merge-me'), after='merge'),
    pr(9, 's/nopr', note('nopr')),
    pr(10, 's/gw-flow', note('gw-flow'), after='merge'),
    *(dict(kind='issue', number=n, **{k: json.loads((FIXTURES / f'issue-{n}.seed.json').read_text())[k]
          for k in ('title', 'body', 'state')}) for n in (11, 12)),
    # F4: the first commit's run must fail before the fix lands on top of it.
    pr(13, 's/fixed', [{'ci/fail': 'the first commit fails\n'}], then=('failed', [{'ci/fail': None}]),
       title='scenario: fixed on top', body='Oracle scenario `s/fixed`: the head passes, the first commit fails.'),
    pr(14, 's/reviewed', note('reviewed'), title='scenario: approved with a comment',
       body='Oracle scenario `s/reviewed`.',
       reviews=[dict(event='APPROVED', body='Looks good.',
                     comments=[dict(path='notes/reviewed.txt', new_position=1, body='A review thread.')])]),
    pr(15, 's/changes', note('changes'), title='scenario: changes requested',
       body='Oracle scenario `s/changes`.', reviews=[dict(event='REQUEST_CHANGES', body='Please change this.')]),
]
BRANCHES = {item['branch'] for item in PLAN if item['kind'] == 'pr'}


class NoRedirects(urllib.request.HTTPRedirectHandler):
    """urllib copies Authorization and Sudo onto any redirect target, so a redirect is an error."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(req.full_url, code, f'refusing redirect to {newurl}', headers, fp)


OPENER = urllib.request.build_opener(NoRedirects)


class TwinError(Exception):
    """An API error, its body and message redacted of the token; `code` is the HTTP status."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


class Twin:
    def __init__(self, base, token):
        self.base, self.token = base.rstrip('/'), token
        self.api_root = self.base + '/api/v1/'

    def allowed(self, method, route, body, sudo):
        """The fence: reads stay inside the repository or the reviewer; each write must have the
        exact route, body and identity the plan uses."""
        repo = f'repos/{REPO}'
        if method == 'GET':
            return sudo is None and (route == repo or route.startswith(repo + '/') or route.startswith(repo + '?')
                                     or route == f'users/{REVIEWER}')
        body = body if isinstance(body, dict) else {}
        if re.fullmatch(rf'{repo}/pulls/\d+/reviews', route):
            return method == 'POST' and sudo == REVIEWER
        if sudo is not None:
            return False
        if (method, route) == ('POST', f'{repo}/pulls'):
            return set(body) == {'base', 'head', 'title', 'body'} and body['base'] == 'main' and body['head'] in BRANCHES
        if (method, route) == ('POST', f'{repo}/issues'):
            return set(body) == {'title', 'body'}
        if (method, route) == ('POST', 'admin/users'):
            return body.get('username') == REVIEWER
        if re.fullmatch(rf'{repo}/pulls/\d+/merge', route):
            return method == 'POST' and body == {'Do': 'merge', 'delete_branch_after_merge': False}
        if re.fullmatch(rf'{repo}/(pulls|issues)/\d+', route):
            return method == 'PATCH' and body == {'state': 'closed'}
        return False

    def redact(self, text):
        return text.replace(self.token, '<token>')

    def api(self, method, route, body=None, sudo=None):
        if not self.allowed(method, route, body, sudo):
            raise SystemExit(f'refusing {method} {route}: outside the twin-seed fence')
        headers = {'Authorization': 'token ' + self.token, 'Content-Type': 'application/json'}
        if sudo: headers['Sudo'] = sudo
        request = urllib.request.Request(self.api_root + route, method=method, headers=headers,
                                         data=None if body is None else json.dumps(body).encode())
        try:
            with OPENER.open(request, timeout=60) as response:
                raw = response.read()
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            # A reply, or a redirect target, can echo what it received; nothing of it may carry the token.
            # Closed here, so no unclosed-response warning prints its message either.
            with error:
                detail = error.read()[:300].decode(errors='replace')
            raise TwinError(error.code, self.redact(f'HTTP {error.code} {error.msg} for {method} {route}: {detail}')) from None
        except (urllib.error.URLError, http.client.HTTPException, OSError, ValueError) as error:
            # A malformed status line or a broken connection can quote the reply too.
            raise TwinError(None, self.redact(f'{type(error).__name__} for {method} {route}: {error}')) from None

    def exists(self, route):
        try:
            self.api('GET', route)
            return True
        except TwinError as error:
            if error.code == 404: return False
            raise

    def git(self, *args, cwd):
        """git without the user's config (no insteadOf to SSH, no hooks or signing), with the token
        in the environment, never in argv or in an error message."""
        header = 'Authorization: Basic ' + base64.b64encode(f'dirtydishes:{self.token}'.encode()).decode()
        # Nothing inherited may point git at another repository or bring back user config.
        inherited = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
        # git would also carry the header to wherever a redirect points, so it follows none.
        env = dict(inherited, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM='1', GIT_TERMINAL_PROMPT='0',
                   GIT_CONFIG_COUNT='2', GIT_CONFIG_KEY_0='http.extraHeader', GIT_CONFIG_VALUE_0=header,
                   GIT_CONFIG_KEY_1='http.followRedirects', GIT_CONFIG_VALUE_1='false')
        result = subprocess.run(['git', *args], cwd=cwd, env=env, capture_output=True, text=True)
        if result.returncode:
            # Some reasons, such as "nothing to commit", arrive on stdout.
            output = (result.stderr + result.stdout).replace(self.token, '<token>').replace(header, '<header>')
            raise SystemExit(f'git {args[0]} failed ({result.returncode}): {output.strip()}')
        return result.stdout.strip()


def existing(twin):
    """Every PR and issue already in the twin, by number, with its title and head branch."""
    found = {}
    for kind in ('pulls', 'issues'):
        page = 1
        while True:
            query = f'state=all&limit=50&page={page}' + ('&type=issues' if kind == 'issues' else '')
            batch = twin.api('GET', f'repos/{REPO}/{kind}?{query}')
            for item in batch:
                head, base = item.get('head') or {}, item.get('base') or {}
                found[item['number']] = (item['title'], head.get('ref'), (head.get('repo') or {}).get('full_name'),
                                         base.get('ref'))
            if len(batch) < 50: break
            page += 1
    return found


def check_resumable(found):
    """An existing PR must match the plan by title, head branch, head repository and base: a PR of the
    same title and branch from a fork, or into another base, is not the plan's."""
    planned = {item['number']: (item['title'], item['branch'], REPO, 'main') if item['kind'] == 'pr'
               else (item['title'], None, None, None) for item in PLAN}
    for number, seen in found.items():
        if planned.get(number) != seen:
            raise SystemExit(f'#{number} {seen!r} is not in the plan; refusing to seed on top of it')
    missing = [n for n in planned if n not in found]
    if missing and any(n > missing[0] for n in found):
        raise SystemExit(f'#{missing[0]} is missing but later numbers exist; the numbering cannot be repaired')


def commit(twin, checkout, changes, message):
    for path, content in changes.items():
        if content is None:
            twin.git('rm', '-q', path, cwd=checkout)
        else:
            (checkout / path).parent.mkdir(parents=True, exist_ok=True)
            (checkout / path).write_text(content)
            twin.git('add', path, cwd=checkout)
    twin.git('commit', '-q', '-m', message, cwd=checkout)


def push(twin, checkout, item):
    if item['branch'] not in BRANCHES: raise SystemExit('branch outside the plan')
    twin.git('push', '-q', 'origin', f'refs/heads/{item["branch"]}:refs/heads/{item["branch"]}', cwd=checkout)


def owned(twin, checkout, item):
    """Whether an existing branch, before its PR opens, holds exactly the plan's changes: the plan's
    commits, rebuilt on the branch's own merge-base with main, give the branch's tree. Comparing
    trees, not messages, catches a commit amended under the plan's message; using the merge-base
    keeps a resume working after main has moved. Anything else stays untouched, and the run stops."""
    twin.git('fetch', '-q', 'origin', f'refs/heads/{item["branch"]}', cwd=checkout)
    remote = twin.git('rev-parse', 'FETCH_HEAD^{tree}', cwd=checkout)
    base = twin.git('merge-base', 'origin/main', 'FETCH_HEAD', cwd=checkout)
    twin.git('switch', '-q', '-C', item['branch'], base, cwd=checkout)
    for i, changes in enumerate(item['commits']):
        commit(twin, checkout, changes, message(item, i + 1))
    return twin.git('rev-parse', 'HEAD^{tree}', cwd=checkout) == remote


def push_branch(twin, checkout, item):
    twin.git('switch', '-q', '-C', item['branch'], 'origin/main', cwd=checkout)
    for i, changes in enumerate(item['commits']):
        commit(twin, checkout, changes, message(item, i + 1))
    push(twin, checkout, item)


def message(item, n):
    """A commit's message. A resumed run recognises its own pushes by it (`pushed_all`), so changing
    this format makes a resume push the later commits again."""
    return item['title'].removeprefix('WIP: ') + f' ({n})'


def pushed_all(twin, item):
    """Whether the branch head is already the plan's last commit. The branch updates at once; a
    PR's commit list lags behind a push."""
    head = twin.api('GET', f'repos/{REPO}/branches/{item["branch"]}')['commit']['message'].strip()
    return head == message(item, len(item['commits']) + len(item['then'][1]))


def push_then(twin, checkout, item):
    """Push the commits that come after the PR opens, unless a resumed run finds them pushed."""
    if pushed_all(twin, item): return
    twin.git('fetch', '-q', 'origin', f'refs/heads/{item["branch"]}', cwd=checkout)
    twin.git('switch', '-q', '-C', item['branch'], 'FETCH_HEAD', cwd=checkout)
    for i, changes in enumerate(item['then'][1]):
        commit(twin, checkout, changes, message(item, len(item['commits']) + i + 1))
    push(twin, checkout, item)
    print(f'pushed the later commits of #{item["number"]}')


def wait(condition, what, timeout):
    deadline = time.monotonic() + timeout
    while not condition():
        if time.monotonic() > deadline: raise SystemExit(f'timed out waiting for {what}')
        time.sleep(5)


def failed(twin, number):
    """Whether the PR head's own CI check has finished failing."""
    head = twin.api('GET', f'repos/{REPO}/pulls/{number}')['head']['sha']
    statuses = twin.api('GET', f'repos/{REPO}/commits/{head}/statuses')
    return any(s['context'] == CHECK and s['status'] == 'failure' for s in statuses)


def reviewer(twin):
    if not twin.exists(f'users/{REVIEWER}'):
        password = base64.b64encode(os.urandom(24)).decode() + '!aA1'
        twin.api('POST', 'admin/users', dict(username=REVIEWER, email=f'{REVIEWER}@users.noreply.invalid',
                 password=password, must_change_password=False, send_notify=False, visibility='public'))
        print(f'created non-admin user {REVIEWER}')
    if twin.api('GET', f'users/{REVIEWER}').get('is_admin') is not False:
        raise SystemExit(f'{REVIEWER} exists but isn\'t a confirmed non-admin; refusing to review as it')


def seed(twin, checkout):
    found = existing(twin)
    check_resumable(found)
    reviewer(twin)
    for item in PLAN:
        number = item['number']
        if number not in found:
            if item['kind'] == 'issue':
                created = twin.api('POST', f'repos/{REPO}/issues', dict(title=item['title'], body=item['body']))
            else:
                if not twin.exists(f'repos/{REPO}/branches/{item["branch"]}'):
                    push_branch(twin, checkout, item)
                elif not owned(twin, checkout, item):
                    raise SystemExit(f'branch {item["branch"]} exists with commits outside the plan; refusing to open #{number} on it')
                created = twin.api('POST', f'repos/{REPO}/pulls', dict(base='main', head=item['branch'],
                                   title=item['title'], body=item['body']))
            if created['number'] != number:
                raise SystemExit(f'Forgejo created #{created["number"]} where the plan expected #{number}; stopping')
            print(f'created #{number} {item["title"]}')
        kind = 'issues' if item['kind'] == 'issue' else 'pulls'
        current = twin.api('GET', f'repos/{REPO}/{kind}/{number}')
        if item.get('then') and item['then'][0] == 'failed':
            if not pushed_all(twin, item):
                print(f"waiting for #{number}'s first run to fail (up to 30 minutes)", flush=True)
                wait(lambda: failed(twin, number), f"#{number}'s first run to fail", timeout=1800)
            push_then(twin, checkout, item)
        if item.get('after') == 'merge' and not current.get('merged'):
            wait(lambda: twin.api('GET', f'repos/{REPO}/pulls/{number}')['mergeable'], f'#{number} to become mergeable', 120)
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
    for item in PLAN:
        if item.get('then') and item['then'][0] == 'last':
            push_then(twin, checkout, item)


def main(argv):
    # Every line reaches a pipe at once, so an interrupted run shows which one-way writes landed.
    sys.stdout.reconfigure(line_buffering=True)
    base = LIVE
    if argv[:1] == ['--rehearse']:
        base = argv[1]
        if urllib.parse.urlsplit(base).hostname not in ('127.0.0.1', 'localhost'):
            raise SystemExit('--rehearse only targets a loopback Forgejo')
    elif argv:
        raise SystemExit(__doc__)
    token = os.environ.get('TWIN_TOKEN') or sys.exit('set TWIN_TOKEN')
    twin = Twin(base, token)
    try:
        with tempfile.TemporaryDirectory() as temp:
            checkout = Path(temp)
            twin.git('clone', '-q', f'{base}/{REPO}.git', str(checkout), cwd=temp)
            twin.git('config', 'user.name', 'dirtydishes', cwd=checkout)
            twin.git('config', 'user.email', 'dirtydishes@users.noreply.invalid', cwd=checkout)
            seed(twin, checkout)
        print(json.dumps({n: seen[0] for n, seen in existing(twin).items()}, indent=2, sort_keys=True))
    except Exception as error:
        # The last line of defence: whatever escaped, its message is redacted and its traceback dropped.
        raise SystemExit(twin.redact(f'{type(error).__name__}: {error}')) from None


if __name__ == '__main__':
    main(sys.argv[1:])
