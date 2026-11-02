"""Check the capture allowlist and ensure expected failures cannot hide new differences."""
import contextlib
import importlib.util
import shutil
import io
import json
from pathlib import Path
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest
from unittest.mock import patch
import subprocess

import compare
import run
import scrub
import writes
spec = importlib.util.spec_from_file_location('compare_writes', Path(__file__).with_name('compare-writes.py'))
compare_writes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare_writes)
spec = importlib.util.spec_from_file_location('compare_viewers', Path(__file__).with_name('compare-viewers.py'))
compare_viewers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compare_viewers)
spec = importlib.util.spec_from_file_location('twin_seed', Path(__file__).with_name('twin-seed.py'))
twin_seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(twin_seed)


class OracleTests(unittest.TestCase):
    def test_write_allowlist_ownership_floor_and_nested_debug(self):
        writes.validate_cases()
        for args in [[], ['pr', 'merge', '1'], ['pr', 'edit', '{number}', '--title', 'other'],
                     ['pr', 'create', '--base', 'main', '--head', 'other']]:
            self.assertFalse(writes.allowed(args))
        with tempfile.TemporaryDirectory() as temp:
            writer = writes.Writer('github.com', Path(temp))
            writer.baseline = 19
            writer.created[18] = 'g3/old'
            with patch.object(writer, 'api') as api:
                for number in (1, 18, 19, 20):
                    with self.assertRaises(ValueError): writer.guard(number)
                    with self.assertRaises(ValueError): writer.register({'id':'create'}, number if number < 20 else 18)
                api.assert_not_called()
            with self.assertRaises(ValueError):
                writer.api('repos/other/repo/git/refs', {}, 'POST')
            with self.assertRaises(ValueError):
                writer.api('repos/' + writes.REPO + '/branches/main/protection', writer.auto_protection(), 'PUT')
        debug = (b'* Request at one\n> Authorization: token secret\n* Request at two\n'
                 b'GraphQL query:\nquery Test { viewer { login } }\n* Request took 1ms\n'
                 b'< Response\nprivate response\n* Request took 2ms\nactual error\n')
        self.assertEqual(writes.without_debug(debug), b'actual error\n')
        with self.assertRaises(RuntimeError): writes.without_debug(b'* Request at incomplete\n')
        with self.assertRaises(RuntimeError): writes.without_debug(b'> Authorization: token secret\n')

    def test_write_comparison_rejects_new_differences_and_fixed_xfail(self):
        expected = Path(__file__).resolve().parents[2] / 'fixtures/oracle/writes'
        with tempfile.TemporaryDirectory() as temp:
            actual = Path(temp)
            for path in expected.iterdir():
                if path.is_file(): shutil.copyfile(path, actual / path.name)
            conflict = actual / 'conflict.stderr'
            conflict.write_text('GraphQL: Pull Request has merge conflicts (mergePullRequest)\n')
            def check():
                with contextlib.redirect_stdout(io.StringIO()):
                    return compare_writes.main(expected, actual)
            self.assertEqual(check(), 0)
            state = actual / 'comment.state.json'
            original = state.read_text()
            state.write_text(original.replace('"body":"x"', '"body":"changed"'))
            self.assertEqual(check(), 1)
            state.write_text(original)
            conflict.write_text(conflict.read_text() + 'unrelated error\n')
            self.assertEqual(check(), 1)
            conflict.write_text((expected / conflict.name).read_text())
            self.assertEqual(check(), 1, 'a fixed xfail must force its removal')
        github = compare_writes.canonical('{"number":27,"headRefName":"g3/create-r4","url":"https://github.com/o/r/pull/27","state":"OPEN"}', {'id':'create'}, {'number':27,'branch':'g3/create-r4'})
        forgejo = compare_writes.canonical('{"number":13,"headRefName":"g3/create","url":"https://forge.test/o/r/pulls/13","state":"OPEN"}', {'id':'create'}, {'number':13,'branch':'g3/create'})
        self.assertEqual(github, forgejo)
        self.assertNotEqual(github, forgejo.replace('OPEN', 'CLOSED'))

    def test_email_scrubbing_at_capture_and_comparison(self):
        raw = b'Author <first.last+tag@sub.example.com>, OTHER@EXAMPLE.ORG; "email":"dirtydishes@users.noreply.invalid"\n'
        expected = b'Author <email@example.invalid>, email@example.invalid; "email":"email@example.invalid"\n'
        self.assertEqual(scrub.emails(raw), expected)
        self.assertEqual(scrub.emails(expected), expected)
        self.assertEqual(compare.normalize(raw.decode(), 'wide-1'), expected.decode())
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp)
            result = subprocess.CompletedProcess([], 0, raw, raw)
            with patch.object(run, 'CASES', [dict(id='view-1', args=['pr', 'view', '1'], exit=0)]), \
                    patch.object(run.subprocess, 'run', return_value=result), \
                    contextlib.redirect_stdout(io.StringIO()):
                run.run('github.com', output)
            self.assertEqual((output / 'view-1.stdout').read_bytes(), expected)
            self.assertEqual((output / 'view-1.stderr').read_bytes(), expected)

    def test_allowlist_rejects_writes_extra_flags_and_shell_text(self):
        for args in [[], ['pr'], ['pr', 'merge', '1'], ['pr', 'view', '8'],
                     ['api', 'repos/dirtydishes/gfj-oracle'], ['pr', 'checks', '1', '--watch'],
                     ['pr', 'view', '1; touch hacked'], ['pr', 'view', '1', '--web'],
                     ['pr', 'view', '1', '--json', 'title; echo secret'],
                     ['pr', 'status', '--something']]:
            self.assertFalse(run.allowed(args), args)
        for case in run.CASES:
            self.assertTrue(run.allowed(case['args']), case['id'])

    def test_debug_capture_keeps_documents_without_headers(self):
        debug = '> Authorization: token secret\nGraphQL query:\nquery Example { viewer { login } }\nGraphQL variables: {"q":"author:@me"}\n\n< Response\n{\n  "query":"query Introspection { __type(name: \"Issue\") { name } }"\n}\n> GET /repos/o/r/readme HTTP/1.1\n'
        requests = run.debug_requests(debug)
        self.assertEqual(requests[0]['variables'], {'q': 'author:@me'})
        self.assertEqual(requests[-1], {'method': 'GET', 'path': '/repos/o/r/readme'})
        self.assertNotIn('secret', json.dumps(requests))

    def test_new_expected_failures_accept_only_rank_or_absent_run_data(self):
        a = '[{"number":1},{"number":2}]'
        b = '[{"number":2},{"number":1}]'
        self.assertTrue(compare.expected_difference('list-search', a, b))
        for wrong in ('[]', '[{"number":2}]', '[{"number":2},{"number":2}]', '[{"number":2,"title":"changed"},{"number":1}]'):
            self.assertFalse(compare.expected_difference('list-search', a, wrong))
        oracle = (Path(__file__).resolve().parents[2] / 'fixtures/oracle/runs.stdout').read_text()
        self.assertTrue(compare.expected_difference('runs', oracle, ''))
        self.assertFalse(compare.expected_difference('runs', oracle, 'unexpected run\n'))
        self.assertFalse(compare.expected_difference('runs', '', ''))

    def test_normalization_preserves_semantics(self):
        a = compare.normalize('{"state":"OPEN","number":1,"detailsUrl":"https://github.com/check","id":"GITHUB_ID","url":"https://github.com/o/r/pull/1","createdAt":"2026-10-30T19:00:00Z"}', 'probe-1')
        b = compare.normalize('{"state":"OPEN","number":1,"detailsUrl":"https://forge.test/check","id":"FGA_ID","url":"https://forge.test/o/r/pulls/1","createdAt":"2020-01-01T00:00:00Z"}', 'probe-1')
        self.assertEqual(a, b)
        self.assertNotEqual(a, b.replace('OPEN', 'CLOSED'))
        self.assertEqual(compare.normalize('check\tpass\t5s\thttps://github.com/check\t\n', 'checks-1'),
                         compare.normalize('check\tpass\t0s\thttps://forge.test/check\t\n', 'checks-1'))

    def test_expected_failure_rejects_unrelated_differences_and_fixed_cases(self):
        root = Path(__file__).resolve().parents[2] / '.local'
        root.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=root) as temp:
            expected, actual = Path(temp) / 'expected', Path(temp) / 'actual'
            expected.mkdir(); actual.mkdir()
            backend = Path(temp) / 'forgejo-pulls.json'
            backend.write_text(json.dumps([{'number': 1, 'head': {'sha': 'a' * 40}}]))
            for case in run.CASES:
                for ext in ('stdout', 'stderr', 'exit'):
                    output = '{"title":"scenario: draft","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE"}\n' if ext == 'stdout' else '0\n' if ext == 'exit' else ''
                    if ext == 'stdout' and case['id'] == 'wide-1':
                        output = json.dumps({'number': 1, 'headRefOid': 'a' * 40, 'url': 'https://forge.test/dirtydishes/gfj-oracle/pull/1'}) + '\n'
                    (expected / f"{case['id']}.{ext}").write_text(output)
                    if ext == 'stdout' and case['id'] == 'probe-4':
                        output = output.replace('CLEAN', 'UNKNOWN').replace('MERGEABLE', 'UNKNOWN')
                    (actual / f"{case['id']}.{ext}").write_text(output)
            def check():
                with contextlib.redirect_stdout(io.StringIO()):
                    with patch.object(compare, 'EXPECTED_FAILURES', {'probe-4': compare.EXPECTED_FAILURES['probe-4']}):
                        return compare.main(expected, actual, backend)
            self.assertEqual(check(), 0)
            draft = actual / 'probe-4.stdout'
            original = draft.read_text()
            draft.write_text(original.replace('scenario: draft', 'WIP: scenario: draft'))
            self.assertEqual(check(), 1, 'draft title differences must fail now')
            draft.write_text(original)
            for case in ('view-4', 'status'):
                path = actual / f'{case}.stdout'
                original = path.read_text()
                path.write_text(original.replace('scenario: draft', 'WIP: scenario: draft'))
                self.assertEqual(check(), 1, f'{case} no longer allows title differences')
                path.write_text(original)
            (actual / 'probe-4.exit').write_text('1\n')
            self.assertEqual(check(), 1, 'a new error cannot pass as an expected draft difference')
            (actual / 'probe-4.exit').write_text('0\n')
            (actual / 'probe-4.stdout').write_text((expected / 'probe-4.stdout').read_text())
            self.assertEqual(check(), 1, 'a fixed case must force removal of its xfail')
            (actual / 'probe-4.stdout').write_text('')
            self.assertEqual(check(), 1, 'missing JSON must produce an unexpected result, not crash')
            report = json.loads((actual / 'report.json').read_text())
            self.assertEqual(next(c for c in report if c['id'] == 'probe-4')['result'], 'unexpected')

    def test_raw_invariants_check_values_the_normalizers_replace(self):
        head = 'a' * 40
        backend = [{'number': 1, 'head': {'sha': head}}]
        def outputs(oid=head, commits=(head,), url='https://forge.test/dirtydishes/gfj-oracle/pull/1'):
            return {'wide-1': json.dumps({'number': 1, 'headRefOid': oid, 'commits': [{'oid': c} for c in commits]}),
                    'list-me-head': json.dumps([{'number': 1, 'url': url}])}
        self.assertEqual(compare.invariants(outputs(), backend), {'head-oid': [], 'pr-urls': []})
        # The SHA normalizer would hide a base SHA served as the head, or commits in Forgejo's order.
        self.assertTrue(compare.invariants(outputs(oid='b' * 40), backend)['head-oid'])
        self.assertTrue(compare.invariants(outputs(commits=(head, 'b' * 40)), backend)['head-oid'])
        # The URL normalizer would hide Forgejo's /pulls/N, which ChatGPT ignores: no expected failure covers it.
        layout = compare.invariants(outputs(url='https://forge.test/dirtydishes/gfj-oracle/pulls/1'), backend)['pr-urls']
        self.assertEqual(layout, ['layout list-me-head: https://forge.test/dirtydishes/gfj-oracle/pulls/1'])
        self.assertNotIn('pr-urls', compare.INVARIANT_FAILURES)
        # A check with nothing to check fails rather than passing vacuously.
        self.assertTrue(compare.invariants({}, backend)['head-oid'])
        self.assertTrue(compare.invariants({}, backend)['pr-urls'])
        # A link to the wrong host is its own problem.
        wrong_host = compare.invariants(outputs(url='https://github.com/dirtydishes/gfj-oracle/pull/1'), backend)['pr-urls']
        self.assertEqual(wrong_host, ['host list-me-head: https://github.com/dirtydishes/gfj-oracle/pull/1'])
        self.assertEqual([compare.classify(*args) for args in [(True, True, False), (True, True, True), (False, True, True), (False, False, True), (False, True, False)]],
                         ['pass', 'unexpected', 'xfail', 'unexpected', 'unexpected'])

    def test_viewer_comparison_allows_only_an_empty_me(self):
        with tempfile.TemporaryDirectory() as temp:
            admin, reader = Path(temp) / 'admin', Path(temp) / 'reader'
            admin.mkdir(); reader.mkdir()
            status = 'Created by you\n  #1  mine\n\nRequesting a code review from you\n'
            for case in run.CASES:
                for ext, value in (('exit', '0\n'), ('stderr', ''), ('stdout', '[{"number":1}]\n')):
                    if ext == 'stdout' and case['id'] == 'status': value = status
                    for root in (admin, reader): (root / f"{case['id']}.{ext}").write_text(value)
            def check():
                with contextlib.redirect_stdout(io.StringIO()):
                    return compare_viewers.main(admin, reader)
            self.assertEqual(check(), 1, 'a declared viewer case that no longer differs must be removed')
            (reader / 'list-me-head.stdout').write_text('[]\n')
            (reader / 'issues-author.stdout').write_text('[]\n')
            (reader / 'status.stdout').write_text(status.replace('  #1  mine', '  You have no open pull requests'))
            self.assertEqual(check(), 0)
            (reader / 'list-me-head.stdout').write_text('[{"number":2}]\n')
            self.assertEqual(check(), 1, 'a viewer case may only become empty')
            (reader / 'list-me-head.stdout').write_text('[]\n')
            (reader / 'list-me-head.exit').write_text('1\n')
            self.assertEqual(check(), 1, 'exit codes must still match')
            (reader / 'list-me-head.exit').write_text('0\n')
            (reader / 'wide-1.stdout').write_text('[]\n')
            self.assertEqual(check(), 1, 'any other case must match exactly')

    def test_protected_viewer_comparison_allows_only_unknown_for_clean_or_unstable(self):
        with tempfile.TemporaryDirectory() as temp:
            admin, reader = Path(temp) / 'admin', Path(temp) / 'reader'
            admin.mkdir(); reader.mkdir()
            states = {'probe-1': 'CLEAN', 'probe-2': 'BLOCKED'}
            for case in run.CASES:
                for ext, value in (('exit', '0\n'), ('stderr', ''), ('stdout', '{"number":1}\n')):
                    if ext == 'stdout' and case['id'] in states:
                        value = json.dumps({'number': 1, 'mergeStateStatus': states[case['id']]}) + '\n'
                    if ext == 'stdout' and case['id'] in compare_viewers.VIEWER_CASES:
                        value = '[]\n' if case['id'] != 'status' else 'Created by you\n  You have no open pull requests\n\nRequesting\n'
                    (admin / f"{case['id']}.{ext}").write_text(value)
                    (reader / f"{case['id']}.{ext}").write_text(value)
            for case in ('list-me-head', 'issues-author'):
                (admin / f'{case}.stdout').write_text('[{"number":1}]\n')
            (admin / 'status.stdout').write_text('Created by you\n  #1  mine\n\nRequesting\n')
            def check():
                with contextlib.redirect_stdout(io.StringIO()):
                    return compare_viewers.main(admin, reader, protected=True)
            self.assertEqual(check(), 1, 'the branch summary must show: no UNKNOWN means it went unread')
            (reader / 'probe-1.stdout').write_text('{"number": 1, "mergeStateStatus": "UNKNOWN"}\n')
            self.assertEqual(check(), 0)
            (reader / 'probe-2.stdout').write_text('{"number": 1, "mergeStateStatus": "UNKNOWN"}\n')
            self.assertEqual(check(), 1, 'a failing required check must block the reader too')
            (reader / 'probe-2.stdout').write_text('{"number": 1, "mergeStateStatus": "BLOCKED"}\n')
            (reader / 'probe-1.stdout').write_text('{"number": 2, "mergeStateStatus": "UNKNOWN"}\n')
            self.assertEqual(check(), 1, 'only the merge state may differ')
    def test_twin_seed_refuses_writes_outside_its_fence_before_the_network(self):
        twin = twin_seed.Twin('https://git.dirtydishes.dev', 'disposable')
        repo = 'repos/' + twin_seed.REPO
        # Twin.api sends through OPENER, so that is what must never be called.
        with patch.object(twin_seed.OPENER, 'open') as send:
            for method, route, body, sudo in [
                ('POST', 'repos/dirtydishes/other/pulls', {}, None),
                ('DELETE', repo + '/issues/1', None, None),
                ('PATCH', repo + '/pulls/1', {'state': 'open'}, None),
                ('PATCH', repo + '/pulls/1', {'title': 'x'}, None),
                ('POST', repo + '/pulls/14/reviews', {}, 'dirtydishes'),
                ('POST', repo + '/pulls/14/reviews', {}, None),
                ('POST', 'admin/users', {'username': 'someone-else'}, None),
                ('POST', 'admin/users', {'username': twin_seed.REVIEWER}, twin_seed.REVIEWER),
                ('GET', 'users/someone-else', None, None),
                ('GET', repo, None, twin_seed.REVIEWER),
                ('POST', repo + '/pulls/6/merge', {'Do': 'merge', 'delete_branch_after_merge': True}, None),
                ('POST', repo + '/pulls/6/merge', {'Do': 'squash', 'delete_branch_after_merge': False}, None),
                ('POST', repo + '/pulls', {'base': 'main', 'head': 'not-planned', 'title': 't', 'body': ''}, None),
                ('POST', repo + '/pulls', {'base': 'other', 'head': 's/pass', 'title': 't', 'body': ''}, None),
                ('POST', repo + '/issues', {'title': 't', 'body': '', 'assignees': ['x']}, None),
            ]:
                with self.assertRaises(SystemExit, msg=f'{method} {route} {body} {sudo}'):
                    twin.api(method, route, body, sudo=sudo)
            send.assert_not_called()
        # The plan's own writes pass the fence.
        for method, route, body, sudo in [
            ('POST', repo + '/pulls', {'base': 'main', 'head': 's/pass', 'title': 't', 'body': ''}, None),
            ('POST', repo + '/issues', {'title': 't', 'body': ''}, None),
            ('POST', repo + '/pulls/6/merge', {'Do': 'merge', 'delete_branch_after_merge': False}, None),
            ('PATCH', repo + '/issues/12', {'state': 'closed'}, None),
            ('POST', repo + '/pulls/14/reviews', {'event': 'APPROVED'}, twin_seed.REVIEWER),
            ('POST', 'admin/users', {'username': twin_seed.REVIEWER}, None),
        ]:
            self.assertTrue(twin.allowed(method, route, body, sudo), f'{method} {route}')

    def test_twin_seed_errors_never_carry_the_token(self):
        twin = twin_seed.Twin('https://git.dirtydishes.dev', 'sekret-token')
        echo = io.BytesIO(b'{"message":"bad","received":"Authorization: token sekret-token"}')
        failure = twin_seed.urllib.error.HTTPError('u', 500, 'Internal sekret-token', {}, echo)
        with patch.object(twin_seed.OPENER, 'open', side_effect=failure):
            with self.assertRaises(twin_seed.TwinError) as raised:
                twin.api('GET', 'repos/' + twin_seed.REPO)
        self.assertEqual(raised.exception.code, 500)
        self.assertNotIn('sekret-token', str(raised.exception))
        # The original HTTPError, which holds the unredacted message, never reaches a traceback.
        self.assertTrue(raised.exception.__suppress_context__)

    def test_twin_seed_opens_a_pr_only_on_its_own_branch(self):
        item = next(i for i in twin_seed.PLAN if i['number'] == 6)
        twin = twin_seed.Twin('https://git.dirtydishes.dev', 'disposable')
        def head(message):
            return patch.object(twin, 'api', return_value={'commit': {'message': message + '\n'}})
        with head(twin_seed.message(item, len(item['commits']))):
            self.assertTrue(twin_seed.owned(twin, item))
        with head('someone else\'s work'):
            self.assertFalse(twin_seed.owned(twin, item))

    def test_twin_seed_resumes_only_onto_its_own_plan(self):
        planned = {item['number']: (item['title'], item.get('branch')) for item in twin_seed.PLAN}
        self.assertEqual(sorted(planned), list(range(1, 16)))
        self.assertEqual(planned[4], ('WIP: scenario: draft', 's/draft'))
        twin_seed.check_resumable({})
        twin_seed.check_resumable({n: planned[n] for n in (1, 2, 3)})
        for found in ({1: ('someone else', 's/pass')}, {1: (planned[1][0], 'other-branch')},
                      {16: ('beyond the plan', None)}, {1: planned[1], 3: planned[3]}):
            with self.assertRaises(SystemExit, msg=str(found)): twin_seed.check_resumable(found)

    def test_twin_seed_never_follows_a_redirect_with_its_token(self):
        request = twin_seed.urllib.request.Request('https://git.dirtydishes.dev/api/v1/user',
                                                   headers={'Authorization': 'token secret'})
        with self.assertRaises(twin_seed.urllib.error.HTTPError):
            twin_seed.NoRedirects().redirect_request(request, None, 302, 'Found', {}, 'https://elsewhere.invalid/')
        handlers = [type(h) for h in twin_seed.OPENER.handlers]
        self.assertIn(twin_seed.NoRedirects, handlers)
        self.assertNotIn(twin_seed.urllib.request.HTTPRedirectHandler, handlers)

    def test_twin_seed_rehearses_only_on_loopback(self):
        with patch.dict(twin_seed.os.environ, {'TWIN_TOKEN': 'disposable'}), \
                patch.object(twin_seed.subprocess, 'run', side_effect=AssertionError('no git')), \
                patch.object(twin_seed.urllib.request, 'urlopen', side_effect=AssertionError('no network')):
            with self.assertRaisesRegex(SystemExit, 'loopback'):
                twin_seed.main(['--rehearse', 'https://git.dirtydishes.dev'])

    def test_twin_seed_keeps_the_token_out_of_argv_errors_and_user_git_config(self):
        token = 'SECRET-TOKEN-123'
        twin = twin_seed.Twin('https://git.dirtydishes.dev', token)
        failure = subprocess.CompletedProcess([], 128, '', f'fatal: rejected for {token}\n')
        with patch.object(twin_seed.subprocess, 'run', return_value=failure) as run:
            with self.assertRaises(SystemExit) as raised:
                twin.git('push', '-q', 'origin', 'refs/heads/s/pass:refs/heads/s/pass', cwd='.')
        argv, env = run.call_args.args[0], run.call_args.kwargs['env']
        self.assertFalse(any(token in arg or 'Authorization' in arg for arg in argv), argv)
        self.assertNotIn(token, str(raised.exception))
        self.assertIn('<token>', str(raised.exception))
        self.assertEqual(env['GIT_CONFIG_GLOBAL'], twin_seed.os.devnull)
        self.assertEqual(env['GIT_CONFIG_NOSYSTEM'], '1')
        self.assertEqual(env['GIT_CONFIG_KEY_0'], 'http.extraHeader')
        self.assertEqual((env['GIT_CONFIG_COUNT'], env['GIT_CONFIG_KEY_1'], env['GIT_CONFIG_VALUE_1']),
                         ('2', 'http.followRedirects', 'false'))


if __name__ == '__main__':
    unittest.main()
