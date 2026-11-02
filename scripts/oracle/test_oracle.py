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
            for case in run.CASES:
                for ext in ('stdout', 'stderr', 'exit'):
                    output = '{"title":"scenario: draft","mergeStateStatus":"CLEAN","mergeable":"MERGEABLE"}\n' if ext == 'stdout' else '0\n' if ext == 'exit' else ''
                    (expected / f"{case['id']}.{ext}").write_text(output)
                    if ext == 'stdout' and case['id'] == 'probe-4':
                        output = output.replace('CLEAN', 'UNKNOWN').replace('MERGEABLE', 'UNKNOWN')
                    (actual / f"{case['id']}.{ext}").write_text(output)
            def check():
                with contextlib.redirect_stdout(io.StringIO()):
                    with patch.object(compare, 'EXPECTED_FAILURES', {'probe-4': compare.EXPECTED_FAILURES['probe-4']}):
                        return compare.main(expected, actual)
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

    def test_twin_seed_refuses_writes_outside_its_fence_before_the_network(self):
        twin = twin_seed.Twin('https://git.dirtydishes.dev', 'disposable')
        repo = 'repos/' + twin_seed.REPO
        with patch.object(twin_seed.urllib.request, 'urlopen') as urlopen:
            for method, route, body, sudo in [
                ('POST', 'repos/dirtydishes/other/pulls', {}, None),
                ('DELETE', repo + '/issues/1', None, None),
                ('PATCH', repo + '/pulls/1', {'state': 'open'}, None),
                ('PATCH', repo + '/pulls/1', {'title': 'x'}, None),
                ('POST', repo + '/pulls/14/reviews', {}, 'dirtydishes'),
                ('POST', 'admin/users', {'username': 'someone-else'}, None),
                ('GET', 'users/someone-else', None, None),
            ]:
                with self.assertRaises(SystemExit, msg=f'{method} {route}'):
                    twin.api(method, route, body, sudo=sudo)
            urlopen.assert_not_called()

    def test_twin_seed_resumes_only_onto_its_own_plan(self):
        planned = {item['number']: (item['title'], item.get('branch')) for item in twin_seed.PLAN}
        self.assertEqual(sorted(planned), list(range(1, 16)))
        self.assertEqual(planned[4], ('WIP: scenario: draft', 's/draft'))
        twin_seed.check_resumable({})
        twin_seed.check_resumable({n: planned[n] for n in (1, 2, 3)})
        for found in ({1: ('someone else', 's/pass')}, {1: (planned[1][0], 'other-branch')},
                      {16: ('beyond the plan', None)}, {1: planned[1], 3: planned[3]}):
            with self.assertRaises(SystemExit, msg=str(found)): twin_seed.check_resumable(found)

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


if __name__ == '__main__':
    unittest.main()
