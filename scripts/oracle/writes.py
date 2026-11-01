"""Fixed gh writes, fenced to fresh g3/* PRs in the oracle twin.

Every capture uses a new numbered branch set. The ledger records ownership before
any subsequent PR write; pre-existing PRs can never enter that ledger. Debug logs
stay in memory. Only scrubbed command output and request documents reach disk.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

from run import debug_requests, REPO
from scrub import emails

ROOT = Path(__file__).resolve().parent
CASES = json.loads((ROOT / 'write-cases.json').read_text())
FIELDS = 'number,url,title,body,state,isDraft,headRefName,baseRefName,autoMergeRequest,comments'


def without_debug(stderr):
    # gh issues parallel requests while deciding whether to merge. Their log
    # blocks can nest, so a single non-greedy regex leaves response lines behind.
    depth = 0
    output = []
    for line in stderr.splitlines(keepends=True):
        if line.startswith(b'* Request at '):
            depth += 1
        if depth:
            if line.startswith(b'* Request took '): depth -= 1
            continue
        if re.match(rb'^\[git [^\n]*\]\n$', line): continue
        output.append(line)
    clean = b''.join(output)
    if depth or re.search(rb'^(?:[><] |GraphQL (?:query|variables):|\* Request)', clean, re.M):
        raise RuntimeError('unrecognized gh debug log; refusing to save it')
    return clean


def allowed(args):
    """Exact command templates, independent of the editable case manifest."""
    return tuple(args) in {
        ('pr', 'create', '--base', 'main', '--head', 'g3/create', '--title', 'T', '--body', 'B'),
        ('pr', 'create', '--draft', '--base', 'main', '--head', 'g3/draft', '--title', 'T', '--body', 'B'),
        ('pr', 'comment', '{number}', '--body', 'x'),
        ('pr', 'edit', '{number}', '--title', 'T2', '--body', 'B2'),
        ('pr', 'edit', '{number}', '--base', 'g3/base-target'),
        ('pr', 'ready', '{number}', '--undo'), ('pr', 'ready', '{number}'),
        ('pr', 'close', '{number}'), ('pr', 'reopen', '{number}'),
        ('pr', 'merge', '{number}', '--merge'),
        ('pr', 'merge', '{number}', '--squash', '--subject', 'G3 squash', '--body', 'G3 body'),
        ('pr', 'merge', '{number}', '--rebase'),
        ('pr', 'merge', '{number}', '--merge', '--match-head-commit', '0' * 40),
        ('pr', 'merge', '{number}', '--merge', '--match-head-commit', '{sha}'),
        ('pr', 'merge', '{number}', '--auto', '--merge'),
        ('pr', 'merge', '{number}', '--disable-auto'),
    }


def validate_cases():
    ids = set()
    for case in CASES:
        if not re.fullmatch(r'[a-z-]+', case['id']):
            raise ValueError('invalid case name')
        for command in case['commands']:
            if not allowed(command['args']) or command['id'] in ids:
                raise ValueError('command outside fixed write allowlist')
            ids.add(command['id'])


class Writer:
    def __init__(self, host, destination):
        validate_cases()
        if host not in ('github.com', 'forge.test'):
            raise ValueError('unsupported write host')
        self.host, self.destination = host, destination
        self.env = dict(os.environ, GH_HOST=host, GH_PROMPT_DISABLED='1', GH_PAGER='cat',
                        NO_COLOR='1', CLICOLOR='0', TZ='UTC')
        self.env.pop('GH_DEBUG', None)
        self.created = {}
        self.baseline = 0
        self.ledger = []
        self.suffix = ''
        self.destination.mkdir(parents=True, exist_ok=True)

    def gh(self, args, *, debug=False, payload=None, check=True):
        result = subprocess.run(['gh', *args], input=None if payload is None else json.dumps(payload).encode(),
            env=dict(self.env, **({'GH_DEBUG':'api'} if debug else {})), capture_output=True, timeout=180)
        if check and result.returncode:
            raise RuntimeError(emails(without_debug(result.stderr) if debug else result.stderr).decode())
        return result

    def api(self, path, payload=None, method='GET'):
        prefix = 'repos/' + REPO + '/'
        if not path.startswith(prefix):
            raise ValueError('repository outside write fence')
        route = path[len(prefix):]
        if method != 'GET':
            if method == 'PUT' and route == 'branches/g3/auto-base/protection':
                if payload != self.auto_protection():
                    raise ValueError('protection outside authorized settings')
            elif method != 'POST' or route not in ('git/trees', 'git/commits', 'git/refs'):
                raise ValueError('setup write outside git-data allowlist')
            if route == 'git/refs' and not re.fullmatch(r'refs/heads/g3/[a-z-]+(?:-r[0-9]+)?', payload['ref']):
                raise ValueError('ref outside g3 fence')
        args = ['api', '--hostname', self.host, '--method', method, path]
        if payload is not None: args += ['--input', '-']
        raw = self.gh(args, payload=payload).stdout
        return json.loads(raw) if raw else None

    def save_ledger(self):
        (self.destination / 'ledger.json').write_text(json.dumps(self.ledger, indent=2) + '\n')

    def branch(self, case):
        return 'g3/' + case + self.suffix

    def github_branch(self, name, base, files):
        if not re.fullmatch(r'g3/[a-z-]+(?:-r[0-9]+)?', name):
            raise ValueError('branch outside g3 fence')
        commit = self.api('repos/' + REPO + '/git/commits/' + base)
        tree = self.api('repos/' + REPO + '/git/trees', dict(base_tree=commit['tree']['sha'],
            tree=[dict(path=p, mode='100644', type='blob', content=v) for p,v in files.items()]), 'POST')
        commit = self.api('repos/' + REPO + '/git/commits',
            dict(message='G3 ' + name, tree=tree['sha'], parents=[base]), 'POST')
        # GitHub refuses an existing ref. Never update or force an existing branch.
        self.api('repos/' + REPO + '/git/refs', dict(ref='refs/heads/' + name, sha=commit['sha']), 'POST')
        self.ledger.append(dict(branch=name, sha=commit['sha'], number=None))
        self.save_ledger()
        return commit['sha']

    def register(self, case, number):
        if number <= self.baseline or number in self.created:
            raise ValueError('refusing to claim an existing PR')
        branch = self.branch(case['id'])
        pr = self.api('repos/' + REPO + f'/pulls/{number}')
        if pr['head']['ref'] != branch or pr['head']['repo']['full_name'] != REPO:
            raise ValueError('created PR has unexpected head')
        self.created[number] = branch
        entry = next(e for e in self.ledger if e['branch'] == branch)
        entry.update(number=number, url=pr['html_url'], state=pr['state'], merged=pr['merged'])
        self.save_ledger()
        print('CREATED ' + pr['html_url'], flush=True)
        if os.environ.get('FGA_LINK_HANDSHAKE') == '1':
            if sys.stdin.readline().strip() != 'linked':
                raise RuntimeError('PR link handshake aborted')

    def guard(self, number):
        if number <= self.baseline or number not in self.created:
            raise ValueError('refusing write to PR not created by this run')
        pr = self.api('repos/' + REPO + f'/pulls/{number}')
        if pr['head']['ref'] != self.created[number] or pr['head']['repo']['full_name'] != REPO:
            raise ValueError('PR head changed outside this run')

    def wait_checks(self, branch, sha, pending=False):
        deadline = time.monotonic() + 180
        while time.monotonic() < deadline:
            runs = self.api('repos/' + REPO + '/actions/runs?event=pull_request&branch=' + branch)['workflow_runs']
            runs = [r for r in runs if r['head_sha'] == sha and r['name'] == 'ci']
            if pending and runs and any(r['status'] != 'completed' for r in runs): return
            if not pending and runs and all(r['status'] == 'completed' for r in runs):
                if any(r['conclusion'] != 'success' for r in runs):
                    raise RuntimeError('G3 checks failed')
                return
            time.sleep(3)
        raise RuntimeError('G3 Actions checks exceeded 180 seconds')

    def capture(self, case, command, number, sha):
        if not allowed(command['args']): raise ValueError('command outside write allowlist')
        if number is not None: self.guard(number)
        args = [a.replace('{number}', str(number)).replace('{sha}', sha) for a in command['args']]
        args = [a + self.suffix if a.startswith('g3/') else a for a in args]
        result = self.gh([*args, '--repo', self.host + '/' + REPO], debug=True, check=False)
        prefix = self.destination / command['id']
        for ext, data in [('stdout', result.stdout), ('stderr', without_debug(result.stderr))]:
            prefix.with_suffix('.' + ext).write_bytes(emails(data))
        prefix.with_suffix('.exit').write_text(str(result.returncode) + '\n')
        prefix.with_suffix('.requests.json').write_bytes(emails((json.dumps(debug_requests(result.stderr.decode()), indent=2) + '\n').encode()))
        if number is None:
            match = re.search(rb'/pulls?/(\d+)', result.stdout)
            if not match: raise RuntimeError('gh did not return a created PR URL')
            number = int(match[1]); self.register(case, number)
        read = self.gh(['pr', 'view', str(number), '--repo', self.host + '/' + REPO, '--json', FIELDS])
        prefix.with_suffix('.state.json').write_bytes(emails(read.stdout))
        prefix.with_suffix('.identity.json').write_text(json.dumps(dict(number=number, branch=self.branch(case['id']))) + '\n')
        print(f"captured write {command['id']}: exit {result.returncode}", flush=True)
        expected = command.get('exit', 0)
        if result.returncode != expected:
            raise RuntimeError(f"{command['id']} exited {result.returncode}, expected {expected}")
        return number

    @staticmethod
    def auto_protection():
        return dict(required_status_checks=dict(strict=False, contexts=['check']),
                    enforce_admins=False, required_pull_request_reviews=None, restrictions=None)

    def capture_conflict_error(self, number):
        # gh rejects DIRTY before sending a mutation. Capture GitHub's mutation
        # error too, since Forgejo's ambiguous boolean makes gh attempt the merge.
        self.guard(number)
        pr = self.api('repos/' + REPO + f'/pulls/{number}')
        payload = dict(query='mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input:$input){clientMutationId}}',
                       variables=dict(input=dict(pullRequestId=pr['node_id'], mergeMethod='MERGE')))
        result = self.gh(['api', '--hostname', self.host, 'graphql', '--input', '-'], payload=payload, check=False)
        data = json.loads(result.stdout)
        if len(data.get('errors', [])) != 1:
            raise RuntimeError('expected a single conflict mutation error')
        error = data['errors'][0]
        if result.returncode != 1 or {key: error.get(key) for key in ('type', 'path', 'message')} != dict(type='UNPROCESSABLE', path=['mergePullRequest'], message='Pull Request has merge conflicts'):
            raise RuntimeError('unexpected GitHub conflict mutation error')
        for name, value in [('conflict-error.json', data), ('conflict-error.requests.json', payload)]:
            (self.destination / name).write_bytes(emails((json.dumps(value, indent=2) + '\n').encode()))

    def github(self):
        # Issues and PRs share numbering, so include both when setting the refusal floor.
        issues = self.api('repos/' + REPO + '/issues?state=all&sort=created&direction=desc&per_page=1')
        self.baseline = max((issue['number'] for issue in issues), default=0)
        branches = {b['ref'].removeprefix('refs/heads/') for b in self.api('repos/' + REPO + '/git/matching-refs/heads/g3/')}
        for run in range(1, 10000):
            self.suffix = '' if run == 1 else '-r' + str(run)
            if not any(self.branch(case) in branches for case in [*(c['id'] for c in CASES), 'base-target', 'conflict-base']): break
        else: raise RuntimeError('no fresh g3 branch set available')
        main = self.api('repos/' + REPO + '/git/ref/heads/main')['object']['sha']
        if 'g3/auto-base' not in branches:
            self.api('repos/' + REPO + '/git/refs', dict(ref='refs/heads/g3/auto-base', sha=main), 'POST')
        self.api('repos/' + REPO + '/branches/g3/auto-base/protection', self.auto_protection(), 'PUT')
        for case in CASES:
            branch = self.branch(case['id'])
            files = {'.g3/' + case['id'] + '.txt': case['id'] + '\n'}
            if case.get('pending'): files['ci/slow'] = '300\n'
            if case.get('conflict'): files['.g3/conflict.txt'] = 'head\n'
            sha = self.github_branch(branch, main, files)
            if case['id'] == 'base':
                self.github_branch(self.branch('base-target'), main, {'.g3/base-target.txt':'base\n'})
            base = 'g3/auto-base' if case.get('pending') else 'main'
            if case.get('conflict'):
                # A common ancestor containing the file is needed for a real conflict.
                # Two independent additions of the same path also produce add/add conflicts.
                base = self.branch('conflict-base')
                self.github_branch(base, main, {'.g3/conflict.txt':'different\n'})
            number = None
            if case['commands'][0]['args'][:2] != ['pr', 'create']:
                result = self.gh(['pr', 'create', '--repo', self.host + '/' + REPO, '--base', base,
                    '--head', branch, '--title', 'T', '--body', 'B'])
                number = int(re.search(rb'/pull/(\d+)', result.stdout)[1])
                self.register(case, number)
            if case.get('wait_checks') or case.get('pending'):
                self.wait_checks(branch, sha, pending=case.get('pending', False))
            if case.get('conflict'):
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    if self.api('repos/' + REPO + f'/pulls/{number}')['mergeable'] is not None: break
                    time.sleep(1)
                else: raise RuntimeError('GitHub conflict calculation exceeded 30 seconds')
            for command in case['commands']:
                number = self.capture(case, command, number, sha)
            if case.get('conflict'):
                self.capture_conflict_error(number)
            pr = self.api('repos/' + REPO + f'/pulls/{number}')
            next(e for e in self.ledger if e.get('number') == number).update(state=pr['state'], merged=pr['merged'])
            self.save_ledger()


if __name__ == '__main__':
    host, destination = sys.argv[1:]
    writer = Writer(host, Path(destination))
    if host == 'github.com': writer.github()
    else: raise SystemExit('Forgejo setup is implemented in scripts/e2e/write-seed.py')
