import os
import json
import pathlib
import re

root = pathlib.Path('/artifacts')
def read(name):
    return json.loads((root / name).read_text())
def check(condition, message):
    assert condition, message
    print('PASS ' + message)

check((root / 'gh-version.txt').read_text().startswith('gh version 2.103.3 '), 'official gh 2.103.3')
check(read('user.json')['login'] == 'e2e', 'gh auth login and gh api user: e2e')
for number, branch, title, draft in [(1, 's/pass', 'scenario: checks pass', False), (2, 's/draft', 'scenario: draft', True)]:
    expected = dict(number=number, title=title, state='OPEN', url=f'https://forge.test/e2e/oracle/pulls/{number}', isDraft=draft, headRefName=branch, baseRefName='main')
    check(read(f'pr{number}.json') == expected, f'PR {number}: exact number/title/state/url/draft/head/base')
headers = (root / 'root.headers').read_text().lower()
check('x-oauth-scopes: repo, read:org, workflow' in headers
      and 'x-github-media-type: github.v3; format=json' in headers
      and read('root.json')['current_user_url'] == 'https://forge.test/api/v3/user'
      and read('meta.json') == {'installed_version': '3.19.0'},
      'API root headers, public URL and GHES version')
repo = read('repo.json')
check(repo['full_name'] == 'e2e/oracle' and repo['private'] and repo['owner']['type'] == 'User' and repo['permissions'] == dict(admin=True, pull=True, push=True), 'Claude repository lookup: GitHub shape and permissions')
lookup = read('lookup.json')
check(len(lookup) == 1 and lookup[0]['number'] == 1 and lookup[0]['head']['label'] == 'e2e:s/pass' and lookup[0]['base']['ref'] == 'main' and lookup[0]['merged_at'] is None, 'Claude head=e2e:s/pass lookup: PR 1')
check(read('wrong-owner.json') == [], 'head owner filter excludes another owner')
check(read('closed.json') == {'state': 'CLOSED'} and read('merged.json') == {'state': 'MERGED'}, 'GraphQL CLOSED and MERGED states')
merged = read('merged-rest.json')
check(len(merged) == 1 and merged[0]['state'] == 'closed' and merged[0]['merged_at'], 'REST merged PR: closed with merged_at')
for name, status, message in [('missing-token', '401', 'Bad credentials'), ('bad-token', '401', 'Bad credentials'), ('unknown-repo', '404', 'Not Found'), ('unknown-route', '404', 'Not Found')]:
    check((root / f'{name}.status').read_text() == status and read(f'{name}.json')['message'] == message, f'{name}: {status} {message}')
for name, kind in [('validation', 'GRAPHQL_VALIDATION_FAILED'), ('not-found-graphql', 'NOT_FOUND'), ('missing-graphql', 'UNAUTHORIZED'), ('invalid-graphql', 'UNAUTHORIZED')]:
    error = read(f'{name}.json')['errors'][0]
    check((root / f'{name}.status').read_text() == '200' and error['type'] == kind and isinstance(error['path'], list) and 'message' in error, f'GraphQL {name}: HTTP 200 and GitHub error shape')
check(read('page1.json')[0]['number'] == 2 and read('page2.json')[0]['number'] == 1 and 'rel="next"' in (root / 'page1.headers').read_text(), 'REST pagination and next Link')
check(all(os.environ['E2E_TOKEN'] not in p.read_text() for p in root.glob('*.debug')), 'gh debug logs redact the disposable token')
requests = []
queries = []
for path in root.glob('*.debug'):
    for line in path.read_text().splitlines():
        match = re.match(r'> (GET|POST|PATCH|PUT|DELETE) (\S+) HTTP/', line)
        if match:
            requests.append(match[2])
    queries.extend(match.group(1) for match in re.finditer(r'GraphQL query:\n(.*?)\nGraphQL variables:', path.read_text(), re.S) if 'PullRequestByNumber' in match.group(1))
check(bool(requests) and all(p.startswith('/api/v3/') or p == '/api/graphql' for p in requests), 'GH_DEBUG requests stay within /api/v3/ and /api/graphql')
check(len(queries) == 2 and queries[0] == queries[1], 'captured identical minimal PullRequestByNumber for both PRs')
query = queries[0]
selection = re.search(r'pullRequest\(number: \$pr_number\) \{([^{}]+)\}', query).group(1)
check(selection.split(',') == ['number', 'title', 'state', 'url', 'isDraft', 'headRefName', 'baseRefName', 'id'], 'minimal selection includes only requested fields plus gh-added id')
check(query == pathlib.Path('/test/gh-pr-view.graphql').read_text().rstrip('\n'), 'captured query matches committed gh 2.103.3 fixture')
(root / 'PullRequestByNumber.graphql').write_text(query)
print('PullRequestByNumber query, verbatim:')
print(query)
