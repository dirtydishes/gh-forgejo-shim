"""Run the shared write cases against the disposable Forgejo seeded by G2."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, '/test/oracle-scripts')
from writes import Writer, CASES, REPO
from scrub import emails

TOKEN = os.environ['GH_ENTERPRISE_TOKEN']
BASE = 'http://forgejo:3000/api/v1/'


def api(path, body=None, method=None):
    request = urllib.request.Request(BASE + path, data=None if body is None else json.dumps(body).encode(),
        method=method, headers={'Authorization': 'token ' + TOKEN, 'Content-Type': 'application/json'})
    with urllib.request.urlopen(request) as response:
        raw = response.read()
        return json.loads(raw) if raw else None


def git(*args):
    return subprocess.check_output(['git', *args], text=True).strip()


class LocalWriter(Writer):
    def api(self, path, payload=None, method='GET'):
        assert method == 'GET' and path.startswith('repos/' + REPO + '/')
        return api(path)


os.chdir('/tmp/g2-seed')
git('-c', 'http.extraHeader=Authorization: token ' + TOKEN, 'fetch', '-q',
    'http://forgejo:3000/' + REPO + '.git', 'main')
main = git('rev-parse', 'FETCH_HEAD')
writer = LocalWriter('forge.test', Path('/artifacts/writes'))
writer.baseline = 12
metadata = api('repos/' + REPO)
Path('/artifacts/write-merge-settings.json').write_text(json.dumps({key: value for key, value in metadata.items() if key.startswith('allow_') or key == 'default_merge_style'}, indent=2) + '\n')


def branch(name, files):
    git('checkout', '-qB', name, main)
    for path, content in files.items():
        target = Path(path); target.parent.mkdir(parents=True, exist_ok=True); target.write_text(content)
    if files:
        git('add', '.'); git('commit', '-qm', 'G3 ' + name)
    sha = git('rev-parse', 'HEAD')
    git('-c', 'http.extraHeader=Authorization: token ' + TOKEN, 'push', '-q',
        'http://forgejo:3000/' + REPO + '.git', 'HEAD:refs/heads/' + name)
    writer.ledger.append(dict(branch=name, sha=sha, number=None)); writer.save_ledger()
    return sha


branch('g3/auto-base', {})
api('repos/' + REPO + '/branch_protections', dict(branch_name='g3/auto-base',
    enable_status_check=True, status_check_contexts=['ci / check (pull_request)'],
    block_on_outdated_branch=False, required_approvals=0), 'POST')
for case in CASES:
    files = {'.g3/' + case['id'] + '.txt': case['id'] + '\n'}
    if case.get('pending'): files['ci/slow'] = '300\n'
    if case.get('conflict'): files['.g3/conflict.txt'] = 'head\n'
    sha = branch(writer.branch(case['id']), files)
    if case['id'] == 'base': branch('g3/base-target', {'.g3/base-target.txt': 'base\n'})
    base = 'g3/auto-base' if case.get('pending') else 'main'
    if case.get('conflict'):
        base = 'g3/conflict-base'; branch(base, {'.g3/conflict.txt': 'different\n'})
    number = None
    if case['commands'][0]['args'][:2] != ['pr', 'create']:
        pull = api('repos/' + REPO + '/pulls', dict(base=base, head=writer.branch(case['id']), title='T', body='B'), 'POST')
        number = pull['number']; writer.register(case, number)
    api('repos/' + REPO + '/statuses/' + sha, dict(context='ci / check (pull_request)',
        state='pending' if case.get('pending') else 'success', description='Has started running',
        target_url='/dirtydishes/gfj-oracle/actions/runs/99/jobs/1'), 'POST')
    # Setup writes can queue mergeability work. Wait after the final status write,
    # and require two settled reads before exercising gh's single mutation.
    if number and not case.get('conflict'):
        settled = 0
        for attempt in range(100):
            settled = settled + 1 if api('repos/' + REPO + f'/pulls/{number}')['mergeable'] else 0
            if settled == 2: break
            time.sleep(0.3)
        else: raise RuntimeError('Forgejo mergeability exceeded 30 seconds')
    if case.get('conflict'): time.sleep(1)
    for command in case['commands']:
        number = writer.capture(case, command, number, sha)
        if case.get('pending'):
            events = api('repos/' + REPO + f'/issues/{number}/timeline')
            # Evidence contains only fields used for auto-merge, with no account metadata.
            evidence = [dict(id=event['id'], type=event['type'], created_at=event['created_at'], body=event['body'], user=dict(login=event['user']['login'])) for event in events if event['type'] in ('pull_scheduled_merge', 'pull_cancel_scheduled_merge')]
            (writer.destination / (command['id'] + '.timeline.json')).write_text(json.dumps(evidence, indent=2) + '\n')
            metadata = api('repos/' + REPO)
            (writer.destination / (command['id'] + '.repository.json')).write_text(json.dumps({key: metadata[key] for key in ('default_merge_style', 'default_branch')}, indent=2) + '\n')
            pull = api('repos/' + REPO + f'/pulls/{number}')
            (writer.destination / (command['id'] + '.pull-keys.json')).write_text(json.dumps(sorted(pull), indent=2) + '\n')
    if case['id'] == 'squash':
        pull = api('repos/' + REPO + f'/pulls/{number}')
        commit = api('repos/' + REPO + '/git/commits/' + pull['merge_commit_sha'])
        assert commit['commit']['message'].strip() == 'G3 squash\n\nG3 body', commit['commit']['message']
        print('PASS squash commit headline and body persisted')
    if case.get('conflict'):
        node = json.loads(writer.gh(['pr', 'view', str(number), '--repo', 'forge.test/' + REPO, '--json', 'id']).stdout)['id']
        payload = dict(query='mutation PullRequestMerge($input:MergePullRequestInput!){mergePullRequest(input:$input){clientMutationId}}',
                       variables=dict(input=dict(pullRequestId=node, mergeMethod='MERGE')))
        result = writer.gh(['api', '--hostname', 'forge.test', 'graphql', '--input', '-'], payload=payload, check=False)
        actual = json.loads(result.stdout)
        expected = json.loads(Path('/test/oracle/writes/conflict-error.json').read_text())
        def errors(value): return [{key: error[key] for key in ('type', 'path', 'message')} for error in value['errors']]
        assert result.returncode == 1 and errors(actual) == errors(expected), actual
        (writer.destination / 'conflict-error.json').write_bytes(emails(result.stdout))
        print('PASS direct conflict mutation matches GitHub type/path/message')
swagger = json.loads(Path('/artifacts/forgejo-swagger.json').read_text())
evidence = dict(
    repository=json.loads((writer.destination / 'auto.repository.json').read_text()),
    pull_fields=json.loads((writer.destination / 'auto.pull-keys.json').read_text()),
    merge_route_methods=sorted(swagger['paths']['/repos/{owner}/{repo}/pulls/{index}/merge']),
    scheduled=json.loads((writer.destination / 'auto.timeline.json').read_text()),
    cancelled=json.loads((writer.destination / 'disable-auto.timeline.json').read_text()),
    scheduled_read_back=json.loads((writer.destination / 'auto.state.json').read_text())['autoMergeRequest'],
    cancelled_read_back=json.loads((writer.destination / 'disable-auto.state.json').read_text())['autoMergeRequest'],
)
Path('/artifacts/forgejo15-auto-merge.json').write_bytes(emails((json.dumps(evidence, indent=2) + '\n').encode()))
print('PASS Forgejo write commands and read-back completed')
