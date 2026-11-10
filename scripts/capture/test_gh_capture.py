"""gh-capture against a fake gh: what it passes through, what it records, and what it never does."""

import importlib.machinery
import importlib.util
import json
import os
import pty
import resource
import select
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

WRAPPER = Path(__file__).resolve().parent / "gh-capture"
# The fake gh: what it does is set by FAKE_GH, so each test asks for the behaviour it needs.
FAKE_GH = r'''#!/usr/bin/env python3
import json, os, signal, sys, time
# Like Go's gh: a write to a closed pipe kills it with SIGPIPE.
signal.signal(signal.SIGPIPE, signal.SIG_DFL)
mode = os.environ.get("FAKE_GH", "echo")
if mode == "echo":
    sys.stdout.write("out " + " ".join(sys.argv[1:]) + "\n")
    sys.stderr.write("err\n")
    sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
if mode == "print":
    sys.stdout.write(os.environ["FAKE_OUT"])
if mode == "fd":
    with open("/dev/fd/" + os.environ["FAKE_FD"]) as handle:
        sys.stdout.write(handle.read())
if mode == "stdin":
    sys.stdout.write(sys.stdin.read())
if mode == "count":
    # Read all of stdin, saying only how much came.
    sys.stderr.write(f"{len(sys.stdin.buffer.read())}\n")
if mode == "late":
    # Something gh starts and leaves running, holding gh's output (an extension's or a shell alias's
    # background job, a browser), that writes after gh has exited.
    print("early", flush=True)
    import subprocess
    subprocess.Popen(["/bin/sh", "-c", "sleep 2; echo late; echo late-err >&2"])
if mode == "late-stderr":
    # The same, holding only stderr.
    print("early", flush=True)
    import subprocess
    subprocess.Popen(["/bin/sh", "-c", "sleep 2; echo late-err >&2"], stdout=subprocess.DEVNULL)
if mode == "interleave":
    for line in range(1, 41):
        (sys.stdout if line % 2 else sys.stderr).write(f"{line}\n")
        (sys.stdout if line % 2 else sys.stderr).flush()
if mode == "ordered":
    # stdout, then stderr, each written and flushed in turn.
    sys.stdout.write("AAAA\n")
    sys.stdout.flush()
    time.sleep(0.2)
    sys.stderr.write("b\n")
if mode == "tty":
    print(json.dumps([os.isatty(0), os.isatty(1), os.isatty(2)]))
if mode == "long":
    sys.stdout.write(os.environ.get("FAKE_PREFIX", "") + "x" * int(os.environ["FAKE_SIZE"]) + os.environ.get("FAKE_SUFFIX", ""))
if mode == "stream":
    # One line, then wait until the caller has read it, as `gh pr checks --watch` does between polls.
    print("first", flush=True)
    sys.stderr.write("progress\n")
    sys.stderr.flush()
    for _ in range(300):
        if os.path.exists(os.environ["FAKE_GO"]):
            break
        time.sleep(0.05)
    print("second")
if mode == "kill":
    sys.stdout.write("dying\n")
    sys.stdout.flush()
    number = int(os.environ["FAKE_SIGNAL"])
    if number != signal.SIGKILL:
        signal.signal(number, signal.SIG_DFL)  # die of it, as gh does, not of Python's KeyboardInterrupt
    os.kill(os.getpid(), number)
if mode == "wait":
    # Note each signal that arrives, then die of it, as gh does.
    def noted(number, frame):
        with open(os.environ["FAKE_NOTE"], "a") as note:
            note.write(f"{number}\n")
        signal.signal(number, signal.SIG_DFL)
        os.kill(os.getpid(), number)
    for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, noted)
    print("ready", flush=True)
    time.sleep(60)
if mode == "flood":
    while True:
        print("line", flush=True)
'''


class Wrapper:
    """A temporary home for one test: the fake gh, a log path, and a way to run gh-capture."""

    def __init__(self, test):
        directory = tempfile.TemporaryDirectory()
        test.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.gh = self.root / "gh"
        self.gh.write_text(FAKE_GH)
        self.gh.chmod(0o755)
        self.log = self.root / "logs" / "calls.jsonl"
        self.env = {"PATH": os.environ["PATH"], "HOME": str(self.root), "GH_CAPTURE_REAL": str(self.gh),
                    "GH_CAPTURE_LOG": str(self.log)}

    def run(self, *argv, **env):
        return subprocess.run([sys.executable, str(WRAPPER), *argv], env=self.env | env, cwd=self.root,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)

    def records(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]


class PassThroughTest(unittest.TestCase):
    def test_gh_answers_unchanged_and_the_call_is_recorded(self):
        wrapper = Wrapper(self)
        result = wrapper.run("pr", "view", "1", FAKE_EXIT="3")
        self.assertEqual((result.returncode, result.stdout, result.stderr), (3, b"out pr view 1\n", b"err\n"))
        [record] = wrapper.records()
        self.assertEqual((record["argv"], record["exit"], record["stdout"], record["stderr"]),
                         (["pr", "view", "1"], 3, "out pr view 1\n", "err\n"))
        self.assertEqual(wrapper.log.stat().st_mode & 0o777, 0o600)
        self.assertTrue(record["chain"] and all(isinstance(command, str) and command for command in record["chain"]))

    def test_the_real_gh_is_never_the_wrapper_or_a_bare_name(self):
        wrapper = Wrapper(self)
        link = wrapper.root / "bin" / "gh"
        link.parent.mkdir()
        link.symlink_to(WRAPPER)
        hard = wrapper.root / "hard-gh"
        os.link(WRAPPER, hard)
        for real in (str(link), str(hard), "gh", "bin/gh", str(wrapper.root / "missing")):
            started = time.monotonic()
            result = wrapper.run("pr", "list", GH_CAPTURE_REAL=real, PATH=f"{link.parent}:{os.environ['PATH']}")
            self.assertLess(time.monotonic() - started, 10, real)
            self.assertEqual((result.returncode, result.stdout), (127, b""), real)
            self.assertIn(b"GH_CAPTURE_REAL", result.stderr, real)
        self.assertFalse(wrapper.log.exists())

    def test_no_ps_and_no_proc_only_cut_the_chain_short(self):
        spec = importlib.util.spec_from_loader("gh_capture", importlib.machinery.SourceFileLoader("gh_capture", str(WRAPPER)))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with mock.patch.object(module.os.path, "exists", return_value=False), \
                mock.patch.object(module.subprocess, "run", side_effect=FileNotFoundError("/bin/ps")):
            self.assertEqual(module.process_chain(), [])


class TerminalTest(unittest.TestCase):
    def test_a_terminal_gets_gh_itself(self):
        wrapper = Wrapper(self)
        controller, terminal = pty.openpty()
        process = subprocess.Popen([sys.executable, str(WRAPPER), "pr", "view", "1"], env=wrapper.env, cwd=wrapper.root,
                                   stdin=subprocess.DEVNULL, stdout=terminal, stderr=subprocess.PIPE)
        os.close(terminal)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        output = b""
        while True:
            ready, _, _ = select.select([controller], [], [], 15)
            self.assertTrue(ready, output)
            try:
                chunk = os.read(controller, 1024)
            except OSError:  # Linux: the terminal's other end has closed
                break
            if not chunk:
                break
            output += chunk
        os.close(controller)
        process.stderr.close()
        self.assertEqual(process.wait(timeout=30), 0)
        self.assertIn(b"out pr view 1", output)
        [record] = wrapper.records()
        self.assertTrue(record["stdout_tty"])
        self.assertNotIn("exit", record)


    def test_a_terminal_stderr_stays_the_terminal(self):
        wrapper = Wrapper(self)
        controller, terminal = pty.openpty()
        self.addCleanup(os.close, controller)
        try:
            result = subprocess.run([sys.executable, str(WRAPPER), "pr", "checks"], env=wrapper.env | {"FAKE_GH": "tty"},
                                    cwd=wrapper.root, stdin=terminal, stdout=subprocess.PIPE, stderr=terminal, timeout=30)
        finally:
            os.close(terminal)
        self.assertEqual((result.returncode, json.loads(result.stdout)), (0, [True, False, True]))
        [record] = wrapper.records()
        self.assertEqual((record["stderr"], record["stderr_tty"]), ({"terminal": True}, True))


class ProbeTest(unittest.TestCase):
    """The fake-token probe: `auth token` for a listed host answers a fake token."""

    def test_every_hostname_form(self):
        wrapper = Wrapper(self)
        (wrapper.root / ".gh-capture").mkdir()
        (wrapper.root / ".gh-capture" / "fake-token-hosts").write_text("# hosts\nforge.test\n")
        for argv in (["auth", "token", "--hostname", "forge.test"], ["auth", "token", "-h", "forge.test"],
                     ["auth", "token", "--hostname=forge.test"]):
            self.assertEqual(wrapper.run(*argv).stdout, b"gfjprobe_forge_test\n", argv)
        self.assertEqual(wrapper.run("auth", "token", "--hostname", "github.com").stdout,
                         b"out auth token --hostname github.com\n")
        self.assertEqual([record.get("faked") for record in wrapper.records()], [True, True, True, None])

    def test_an_unreadable_host_list_falls_through_to_gh(self):
        wrapper = Wrapper(self)
        hosts = wrapper.root / ".gh-capture" / "fake-token-hosts"
        hosts.mkdir(parents=True)
        result = wrapper.run("auth", "token", "--hostname", "forge.test")
        self.assertEqual((result.returncode, result.stdout), (0, b"out auth token --hostname forge.test\n"))
        hosts.rmdir()
        hosts.write_bytes(b"\xff\xfe forge.test\n")
        result = wrapper.run("auth", "token", "--hostname", "forge.test")
        self.assertEqual((result.returncode, result.stdout), (0, b"out auth token --hostname forge.test\n"))


class InputTest(unittest.TestCase):
    """What the caller hands gh reaches it: stdin, and files opened for it (`--body-file <(...)`)."""

    def test_a_file_the_caller_opened_for_gh(self):
        wrapper = Wrapper(self)
        read_end, write_end = os.pipe()
        os.write(write_end, b"the body\n")
        os.close(write_end)
        self.addCleanup(os.close, read_end)
        result = subprocess.run([sys.executable, str(WRAPPER), "pr", "create", "--body-file", f"/dev/fd/{read_end}"],
                                env=wrapper.env | {"FAKE_GH": "fd", "FAKE_FD": str(read_end)}, cwd=wrapper.root,
                                pass_fds=(read_end,), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=30)
        self.assertEqual((result.returncode, result.stdout), (0, b"the body\n"), result.stderr)

    def test_stdin(self):
        wrapper = Wrapper(self)
        result = subprocess.run([sys.executable, str(WRAPPER), "secret", "set", "NAME"], env=wrapper.env | {"FAKE_GH": "stdin"},
                                cwd=wrapper.root, input=b"value\n", stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                timeout=30)
        self.assertEqual(result.stdout, b"value\n")


    def run_with_input(self, wrapper, argv, data, mode="stdin"):
        return subprocess.run([sys.executable, str(WRAPPER), *argv], env=wrapper.env | {"FAKE_GH": mode}, cwd=wrapper.root,
                              input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)

    def test_stdin_gh_was_told_to_read_is_recorded_without_credentials(self):
        wrapper = Wrapper(self)
        body = b'{"title":"t","token":"S30","note":"see ghp_' + b"S31" * 12 + b'"}'
        for argv, data in ((["pr", "create", "--body-file", "-"], b"the body\n"), (["api", "repos/o/r/issues", "--input", "-"], body),
                           (["api", "graphql", "-F", "query=@-"], b"{ viewer { login } }")):
            self.assertEqual(self.run_with_input(wrapper, argv, data).stdout, data, argv)
        first, second, third = wrapper.records()
        self.assertEqual((first["stdin"], third["stdin"]), ("the body\n", "{ viewer { login } }"))
        self.assertEqual(json.loads(second["stdin"]), {"title": "t", "token": "<redacted>", "note": "see <token>"})

    def test_credential_input_is_never_recorded(self):
        wrapper = Wrapper(self)
        for argv, data in ((["auth", "login", "--with-token"], TOKEN), (["secret", "set", "-f", "-"], "NAME=S32\n"),
                           (["auth", "git-credential", "get"], "protocol=https\nhost=forge.test\npassword=S33\n"),
                           (["api", "x", "-F", "token=@-"], "S34\n"), (["tok", "-"], "S35\n")):
            result = self.run_with_input(wrapper, argv, data.encode(), mode="count")
            self.assertEqual(result.stderr, f"{len(data)}\n".encode(), argv)
        log = wrapper.log.read_text()
        for secret in (TOKEN.strip(), "S32", "S33", "S34", "S35"):
            self.assertNotIn(secret, log)
        self.assertEqual([record.get("stdin") for record in wrapper.records()],
                         [{"redacted": True}, {"redacted": True}, None, {"redacted": True}, {"redacted": True}])

    def test_stdin_gh_wasnt_told_to_read_stays_unread(self):
        wrapper = Wrapper(self)
        lines = wrapper.root / "lines"
        lines.write_text("one\ntwo\n")
        # A `-` that's a value gh uses as it is: a raw field, a body, a title; and `-F -` where `-F` is a field.
        for argv in (["pr", "view"], ["api", "x", "-f", "title=-"], ["api", "x", "--raw-field=title=-"], ["pr", "create", "--body", "-"],
                     ["pr", "create", "-t", "-"], ["issue", "create", "-b-"], ["api", "x", "-F", "-"],
                     ["workflow", "run", "w", "-f", "x=@-"], ["pr", "checkout", "-"]):
            with open(lines, "rb") as stdin:
                result = subprocess.run(["/bin/sh", "-c", '"$@" >/dev/null 2>&1; cat', "sh", sys.executable, str(WRAPPER), *argv],
                                        env=wrapper.env, cwd=wrapper.root, stdin=stdin, stdout=subprocess.PIPE, timeout=30)
            self.assertEqual(result.stdout, b"one\ntwo\n", argv)
        self.assertFalse(any("stdin" in record for record in wrapper.records()))

    def test_every_way_gh_reads_stdin(self):
        wrapper = Wrapper(self)
        calls = (["issue", "create", "-F", "-"], ["issue", "comment", "1", "-F-"], ["release", "create", "v1", "--notes-file=-"],
                 ["pr", "merge", "--body-file", "-"], ["agent-task", "create", "-F", "-"], ["workflow", "run", "w", "-F", "x=@-"],
                 ["api", "x", "--field=x=@-"], ["api", "x", "-Fx=@-"], ["api", "x", "--input=-"], ["gist", "create", "-"],
                 ["gist", "new", "--desc", "d", "-"], ["alias", "set", "co2", "-"], ["alias", "import", "-"],
                 ["gist", "edit", "1", "-"], ["ssh-key", "add", "-"], ["repo", "deploy-key", "add", "-"])
        for argv in calls:
            self.assertEqual(self.run_with_input(wrapper, list(argv), b"in\n").stdout, b"in\n", argv)
        self.assertEqual([record.get("stdin") for record in wrapper.records()], ["in\n"] * len(calls))


class ArgvTest(unittest.TestCase):
    """#197 defect 1: no credential in a call's argv reaches the log."""

    def test_credentials_in_argv(self):
        wrapper = Wrapper(self)
        calls = [
            ["api", "-H", "Authorization: token S1", "repos/o/r"],
            ["api", "-f", "token=S2", "--field", "password=S3", "-F", "client_secret=S4", "repos/o/r"],
            ["secret", "set", "NAME", "--body", "S5"], ["secret", "set", "NAME", "-b", "S6"],
            ["secret", "set", "NAME", "--body=S7"], ["-R", "o/r", "secret", "set", "NAME", "-bS8"],
            ["api", "--token", "S9", "user"], ["api", "https://octo:S10@forge.test/api/v3/user"],
            ["api", "user", "-H", "X-Api-Key: S11"], ["api", "-f", "key=ghp_" + "S12" * 12],
        ]
        for argv in calls:
            self.assertEqual(wrapper.run(*argv, FAKE_GH="print", FAKE_OUT="ok\n").returncode, 0)
        log = wrapper.log.read_text()
        for secret in ("S1", "S2", "S3", "S4", "S5", "S6", "S7", "S8", "S9", "S10", "S11", "S12"):
            self.assertNotRegex(log, secret + r"\b", secret)
        argvs = [record["argv"] for record in wrapper.records()]
        self.assertEqual(argvs[0], ["api", "-H", "Authorization: token <token>", "repos/o/r"])
        self.assertEqual(argvs[1], ["api", "-f", "token=<redacted>", "--field", "password=<redacted>", "-F",
                                    "client_secret=<redacted>", "repos/o/r"])
        self.assertEqual([argvs[2][-1], argvs[3][-1], argvs[4][-1], argvs[5][-1]],
                         ["<redacted>", "<redacted>", "--body=<redacted>", "-b<redacted>"])
        self.assertEqual(argvs[6], ["api", "--token", "<redacted>", "user"])

    def test_a_secret_body_behind_gh_secrets_own_flags(self):
        wrapper = Wrapper(self)
        for argv in (["--org", "o", "secret", "set", "NAME", "--body", "S13"], ["-e", "prod", "secret", "set", "NAME", "-b", "S14"],
                     ["--env=prod", "secret", "set", "NAME", "--body", "S15"]):
            wrapper.run(*argv, FAKE_GH="print", FAKE_OUT="ok\n")
        log = wrapper.log.read_text()
        for secret in ("S13", "S14", "S15"):
            self.assertNotIn(secret, log)

    def test_a_field_with_a_sensitive_name_in_any_part(self):
        wrapper = Wrapper(self)
        calls = [
            ["api", "repos/o/r/hooks", "-f", "config[secret]=S16"], ["api", "repos/o/r/hooks", "-fconfig[secret]=S17"],
            ["api", "repos/o/r/hooks", "--raw-field=config[secret]=S18"], ["api", "x", "-F", "a[b][api_key]=S19"],
            ["api", "x", "--field", "config[password][]=S20"], ["api", "x", "-f=config[Token]=S21"],
            ["workflow", "run", "w", "-f", "inputs[secret]=has space S22"], ["api", "x", "-F", "secret[]=S23"],
        ]
        for argv in calls:
            wrapper.run(*argv, FAKE_GH="print", FAKE_OUT="ok\n")
        log = wrapper.log.read_text()
        for secret in ("S16", "S17", "S18", "S19", "S20", "S21", "S22", "S23"):
            self.assertNotIn(secret, log)
        argvs = [record["argv"] for record in wrapper.records()]
        self.assertEqual([argvs[0][-1], argvs[1][-1], argvs[2][-1], argvs[5][-1]],
                         ["config[secret]=<redacted>", "-fconfig[secret]=<redacted>",
                          "--raw-field=config[secret]=<redacted>", "-f=config[Token]=<redacted>"])

    def test_every_field_sent_to_a_secret_or_variable_path(self):
        wrapper = Wrapper(self)
        calls = [
            ["api", "-X", "PUT", "repos/o/r/actions/secrets/X", "-f", "data=S40"],
            ["api", "-X", "POST", "repos/o/r/actions/variables", "-f", "name=V", "-f", "value=S41"],
            ["api", "-X", "PATCH", "repos/o/r/actions/variables/V", "-fvalue=S42", "--raw-field=value=S43", "--field", "value=S44"],
            ["api", "-X", "PUT", "orgs/o/actions/secrets/X", "-F", "data=S45"],
        ]
        for argv in calls:
            wrapper.run(*argv, FAKE_GH="print", FAKE_OUT="ok\n")
        log = wrapper.log.read_text()
        for secret in ("S40", "S41", "S42", "S43", "S44", "S45"):
            self.assertNotIn(secret, log)
        argvs = [record["argv"] for record in wrapper.records()]
        self.assertEqual(argvs[1], ["api", "-X", "POST", "repos/o/r/actions/variables", "-f", "name=<redacted>", "-f",
                                    "value=<redacted>"])
        self.assertEqual(argvs[2][-4:], ["-fvalue=<redacted>", "--raw-field=value=<redacted>", "--field", "value=<redacted>"])
        self.assertEqual(argvs[3], ["api", "-X", "PUT", "orgs/o/actions/secrets/X", "-F", "data=<redacted>"])
        result = subprocess.run([sys.executable, str(WRAPPER), "api", "repos/o/r/actions/variables", "--input", "-"],
                                env=wrapper.env | {"FAKE_GH": "count"}, cwd=wrapper.root, input=b'{"value":"S46"}',
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(result.stderr, b"15\n")
        self.assertEqual(wrapper.records()[-1]["stdin"], {"redacted": True})
        self.assertNotIn("S46", wrapper.log.read_text())

    def test_ordinary_fields_stay(self):
        wrapper = Wrapper(self)
        argv = ["api", "repos/o/r/hooks", "-f", "config[url]=https://hook.test/x", "-F", "events[]=push", "-f", "name=web"]
        wrapper.run(*argv)
        self.assertEqual(wrapper.records()[0]["argv"], argv)

    def test_ordinary_argv_stays(self):
        wrapper = Wrapper(self)
        argv = ["pr", "list", "--head", "s/pass", "--json", "number,url", "--repo", "forge.test/o/r", "--body", "b"]
        wrapper.run(*argv)
        self.assertEqual(wrapper.records()[0]["argv"], argv)


TOKEN = "gho_" + "Tk9" * 12 + "\n"


class TokenOutputTest(unittest.TestCase):
    """#197 defects 2 and 3: output that can hold a token is never stored, only its length."""

    def hidden(self, *argv, output=TOKEN):
        wrapper = Wrapper(self)
        result = wrapper.run(*argv, FAKE_GH="print", FAKE_OUT=output)
        self.assertEqual(result.stdout, output.encode(), argv)
        [record] = wrapper.records()
        self.assertEqual(record["stdout"], {"bytes": len(output), "redacted": True}, argv)
        self.assertNotIn(output.strip(), wrapper.log.read_text())

    def test_auth_token_wherever_its_flags_sit(self):
        self.hidden("auth", "token")
        self.hidden("auth", "token", "--hostname", "forge.test")
        self.hidden("--hostname", "forge.test", "auth", "token")
        self.hidden("-h", "forge.test", "auth", "token")

    def test_auth_status_showing_the_token(self):
        for flag in ("--show-token", "--show-token=true", "-t", "-at"):
            self.hidden("auth", "status", flag)

    def test_other_commands_that_print_tokens(self):
        self.hidden("config", "get", "oauth_token")
        self.hidden("config", "get", "-h", "forge.test", "oauth_token")
        self.hidden("config", "list", "-h", "forge.test")
        self.hidden("api", "-X", "POST", "repos/o/r/actions/runners/registration-token",
                    output='{"token":"AABBCCDDEEFF","expires_at":"2026-10-30T20:17:03Z"}')
        self.hidden("api", "--method", "POST", "/app/installations/1/access_tokens", output='{"token":"ghs_x"}')
        self.hidden("auth", "git-credential", "get", output="protocol=https\nhost=forge.test\npassword=S1\n")

    def test_aliases_extensions_and_more_token_paths(self):
        self.hidden("tok")
        self.hidden("my-extension", "--flag")
        self.hidden("co", "1")
        self.hidden("extension", "exec", "my-extension")
        self.hidden("api", "-X", "POST", "orgs/o/actions/runners/generate-jitconfig", output='{"encoded_jit_config":"e30="}')
        self.hidden("api", "repos/o/r/actions/runners/registration-token?per_page=1", output='{"token":"AABB"}')
        self.hidden("api", "user/applications/oauth2", "--jq", ".client_secret", output="gto_" + "q" * 30 + "\n")
        self.hidden("pr", "view", "--template", "{{.token}}")
        self.hidden("api", "user/applications/oauth2", "-q.client_secret", output="s3cr3t\n")
        self.hidden("pr", "view", "-t{{.token}}", output="s3cr3t\n")

    def test_credentials_in_headers_cut_short_answers_and_new_prefixes(self):
        wrapper = Wrapper(self)
        answer = 'HTTP/2.0 201 Created\r\nContent-Type: application/json\r\n\r\n{"client_id":"c1","client_secret":"gto_' + "q" * 30 + '"}'
        wrapper.run("api", "-i", "-X", "POST", "user/applications/oauth2", FAKE_GH="print", FAKE_OUT=answer)
        wrapper.run("api", "repos/o/r/contents", FAKE_GH="long", FAKE_SIZE="300000", FAKE_PREFIX='{"token": "AABBCC", ')
        first, second = wrapper.records()
        self.assertIn('"client_secret":"<redacted>"', first["stdout"])
        self.assertNotIn("gto_", first["stdout"])
        self.assertTrue(second["stdout"]["head"].startswith('{"token": "<redacted>", '))

    def test_a_sensitive_value_cut_short_goes_to_the_cut(self):
        wrapper = Wrapper(self)
        prefix = '{"patch":"'
        for into in ('","client_secret":"', '","client_secret": "ab\\"', '","token":"'):
            size = 256 * 1024 - len(prefix) - len(into) - 10  # the cut falls ten bytes into the value
            wrapper.run("api", "repos/o/r/contents", FAKE_GH="long", FAKE_SIZE=str(size), FAKE_PREFIX=prefix,
                        FAKE_SUFFIX=into + "Q" * 40 + '"}')
        for record, into in zip(wrapper.records(), ('","client_secret":"', '","client_secret": "', '","token":"')):
            head = record["stdout"]["head"]
            self.assertTrue("QQQ" not in head and head.endswith(into + "<redacted>"), head[-60:])

    def test_scrubbing_is_quick_and_keeps_empty_values(self):
        wrapper = Wrapper(self)
        patch = "a token and a secret and a password " * 3000  # under Linux's 128 KiB for one variable
        answer = 'HTTP/2.0 200 OK\r\n\r\n{"temp_clone_token":"","patch":"' + patch + '","token":"ab"}'
        started = time.monotonic()
        wrapper.run("api", "-i", "repos/o/r/pulls/1/files", FAKE_GH="print", FAKE_OUT=answer)
        self.assertLess(time.monotonic() - started, 2)
        stored = wrapper.records()[0]["stdout"]
        self.assertIn('"temp_clone_token":""', stored)
        self.assertIn('"token":"<redacted>"', stored)
        self.assertIn(patch, stored)

    def test_an_answer_full_of_quotes_is_scrubbed_quickly(self):
        wrapper = Wrapper(self)
        # A diff of JSON files: escaped quotes by the thousand, in one string.
        answer = '[{"filename":"a.json","patch":"' + '\\"x' * 30000 + '"}]'
        started = time.monotonic()
        wrapper.run("api", "repos/o/r/pulls/1/files", FAKE_GH="print", FAKE_OUT=answer)
        self.assertLess(time.monotonic() - started, 2)
        self.assertEqual(wrapper.records()[0]["stdout"], answer)
        # The same, cut short at 256 KiB inside that string.
        started = time.monotonic()
        wrapper.run("api", "repos/o/r/pulls/1/files", FAKE_GH="long", FAKE_SIZE="300000",
                    FAKE_PREFIX='[{"filename":"a.json","patch":"' + '\\"x' * 20000)
        self.assertLess(time.monotonic() - started, 2)
        self.assertTrue(wrapper.records()[1]["stdout"]["head"].startswith('[{"filename":"a.json","patch":"\\"x\\"x'))

    def test_commands_and_their_aliases_keep_their_output(self):
        wrapper = Wrapper(self)
        for argv in (["discussion", "list"], ["skill", "list"], ["cs", "list"], ["environment", "list"]):
            wrapper.run(*argv, FAKE_GH="print", FAKE_OUT="[]\n")
        self.assertEqual([record["stdout"] for record in wrapper.records()], ["[]\n"] * 4)

    def test_command_words_not_any_word(self):
        wrapper = Wrapper(self)
        wrapper.run("pr", "create", "--title", "secret", "--body", "the body")
        wrapper.run("search", "issues", "config", "get", FAKE_GH="print", FAKE_OUT="[]\n")
        first, second = wrapper.records()
        self.assertEqual(first["argv"][-1], "the body")
        self.assertEqual(second["stdout"], "[]\n")

    def test_any_other_output_loses_its_credentials(self):
        wrapper = Wrapper(self)
        answer = '{"login":"octo","token":"AABBCC","nested":{"client_secret":"S2"}}'
        wrapper.run("api", "user", FAKE_GH="print", FAKE_OUT=answer + "\n")
        wrapper.run("pr", "view", FAKE_GH="print", FAKE_OUT="see " + TOKEN)
        first, second = wrapper.records()
        self.assertEqual(json.loads(first["stdout"]), {"login": "octo", "token": "<redacted>",
                                                       "nested": {"client_secret": "<redacted>"}})
        self.assertEqual(second["stdout"], "see <token>\n")


class StreamTest(unittest.TestCase):
    """#197 defect 4: output reaches the caller as gh writes it, not when gh exits."""

    def test_a_line_arrives_while_gh_runs(self):
        wrapper = Wrapper(self)
        go = wrapper.root / "go"
        process = subprocess.Popen([sys.executable, str(WRAPPER), "pr", "checks", "1", "--watch"],
                                   env=wrapper.env | {"FAKE_GH": "stream", "FAKE_GO": str(go)}, cwd=wrapper.root,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(process.kill)
        ready, _, _ = select.select([process.stdout], [], [], 10)
        self.assertTrue(ready, "no output while gh ran")
        self.assertEqual(process.stdout.readline(), b"first\n")
        self.assertIsNone(process.poll())
        go.touch()
        rest, errors = process.communicate(timeout=30)
        self.assertEqual((process.returncode, rest, errors), (0, b"second\n", b"progress\n"))
        [record] = wrapper.records()
        self.assertEqual((record["stdout"], record["stderr"]), ("first\nsecond\n", "progress\n"))

    def test_a_slow_caller_on_a_non_blocking_pipe_gets_everything(self):
        wrapper = Wrapper(self)
        read_end, write_end = os.pipe()
        os.set_blocking(write_end, False)
        process = subprocess.Popen([sys.executable, str(WRAPPER), "api", "repos/o/r/tarball"],
                                   env=wrapper.env | {"FAKE_GH": "long", "FAKE_SIZE": "2000000"}, cwd=wrapper.root,
                                   stdin=subprocess.DEVNULL, stdout=write_end, stderr=subprocess.DEVNULL)
        os.close(write_end)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        received = 0
        with os.fdopen(read_end, "rb") as pipe:
            while True:
                # A wrapper that stops forwarding hangs: fail after a quiet spell, don't wait forever.
                ready, _, _ = select.select([pipe], [], [], 15)
                self.assertTrue(ready, f"output stopped after {received} bytes")
                chunk = pipe.read1(4096)
                if not chunk:
                    break
                received += len(chunk)
                time.sleep(0.0005)
        self.assertEqual((process.wait(timeout=30), received), (0, 2000000))

    @unittest.skipUnless(os.path.exists("/dev/full"), "needs /dev/full (Linux)")
    def test_a_caller_that_cant_take_the_output(self):
        wrapper = Wrapper(self)
        with open("/dev/full", "wb") as full:
            result = subprocess.run([sys.executable, str(WRAPPER), "api", "user"], env=wrapper.env | {"FAKE_GH": "long",
                                    "FAKE_SIZE": "500000"}, cwd=wrapper.root, stdin=subprocess.DEVNULL, stdout=full,
                                    stderr=subprocess.DEVNULL, timeout=30)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(wrapper.records()[0]["stdout"]["bytes"], 500000)

    def test_a_stdout_that_cant_be_written_fails_only_what_gh_fails(self):
        wrapper = Wrapper(self)
        unwritable = wrapper.root / "read-only"
        unwritable.write_text("")
        # `gh api` checks its writes and fails; most commands, such as `gh pr view`, don't.
        for argv, code in ((["api", "user"], 1), (["pr", "diff"], 1), (["pr", "view"], 0), (["run", "list"], 0)):
            with open(unwritable, "rb") as stdout:
                result = subprocess.run([sys.executable, str(WRAPPER), *argv], env=wrapper.env | {"FAKE_GH": "long",
                                        "FAKE_SIZE": "500000"}, cwd=wrapper.root, stdin=subprocess.DEVNULL, stdout=stdout,
                                        stderr=subprocess.PIPE, timeout=30)
            self.assertEqual(result.returncode, code, argv)
            self.assertEqual(b"gh-capture: stdout" in result.stderr, bool(code), argv)
        for record in wrapper.records():
            self.assertEqual((record["exit"], record["stdout"]["bytes"], record["forward_failed"]), (0, 500000, ["stdout"]))

    def test_one_place_for_both_streams_keeps_their_order(self):
        wrapper = Wrapper(self)
        for _ in range(5):
            result = subprocess.run([sys.executable, str(WRAPPER), "pr", "checks"], env=wrapper.env | {"FAKE_GH": "interleave"},
                                    cwd=wrapper.root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, timeout=30)
            self.assertEqual(result.stdout, "".join(f"{line}\n" for line in range(1, 41)).encode())
        record = wrapper.records()[0]
        self.assertEqual((record["stderr"], record["stdout"].count("\n")), ({"merged_into_stdout": True}, 40))

    def test_a_file_opened_twice_or_shared_gets_what_gh_alone_writes(self):
        wrapper = Wrapper(self)
        for redirect, mode in ((">f 2>f", "ordered"), (">f 2>&1", "ordered"), (">f 2>&1", "interleave"),
                               (">>f 2>>f", "interleave")):
            got = {}
            for name, command in (("direct", [str(wrapper.gh)]), ("wrapped", [sys.executable, str(WRAPPER)])):
                (wrapper.root / "f").write_text("")
                subprocess.run(["/bin/sh", "-c", f'"$@" pr checks {redirect}', "sh", *command], env=wrapper.env | {"FAKE_GH": mode},
                               cwd=wrapper.root, stdin=subprocess.DEVNULL, timeout=30, check=True)
                got[name] = (wrapper.root / "f").read_text()
            self.assertEqual(got["wrapped"], got["direct"], redirect)

    def test_what_gh_leaves_running_reaches_the_caller_after_gh_exits(self):
        wrapper = Wrapper(self)
        started = time.monotonic()
        process = subprocess.Popen([sys.executable, str(WRAPPER), "pr", "view", "--web"], env=wrapper.env | {"FAKE_GH": "late"},
                                   cwd=wrapper.root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        # The caller sees gh's exit when gh exits, not when what it left running does.
        self.assertEqual(process.wait(timeout=30), 0)
        self.assertLess(time.monotonic() - started, 1.8)
        # And everything written to the streams gh handed on, as it would without the wrapper.
        out, errors = process.stdout.read(), process.stderr.read()
        process.stdout.close()
        process.stderr.close()
        self.assertEqual((out, errors), (b"early\nlate\n", b"late-err\n"))
        # The record waits for the end of that output.
        for _ in range(100):
            if wrapper.log.exists():
                break
            time.sleep(0.05)
        [record] = wrapper.records()
        self.assertEqual((record["exit"], record["stdout"], record["stderr"]), (0, "early\nlate\n", "late-err\n"))

    def test_what_gh_leaves_holding_one_stream_holds_only_that_one(self):
        wrapper = Wrapper(self)
        started = time.monotonic()
        # `$(…)` waits for its pipe to close: with the job holding only stderr, that's when gh exits.
        result = subprocess.run(["/bin/sh", "-c", 'out=$("$@" pr view 2>/dev/null); echo "[$out]"', "sh", sys.executable,
                                 str(WRAPPER)], env=wrapper.env | {"FAKE_GH": "late-stderr"}, cwd=wrapper.root,
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, timeout=30)
        self.assertEqual(result.stdout, b"[early]\n")
        self.assertLess(time.monotonic() - started, 1.8)
        for _ in range(100):
            if wrapper.log.exists():
                break
            time.sleep(0.05)
        self.assertEqual(wrapper.records()[0]["stderr"], "late-err\n")

    def test_what_passes_on_the_rest_stops_on_sigterm_and_records(self):
        wrapper = Wrapper(self)
        marker = wrapper.root.name
        process = subprocess.Popen([sys.executable, str(WRAPPER), "pr", "view", marker], env=wrapper.env | {"FAKE_GH": "late"},
                                   cwd=wrapper.root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        self.addCleanup(process.stdout.close)
        self.addCleanup(process.stderr.close)
        self.assertEqual(process.wait(timeout=30), 0)
        [child] = [pid for pid, command in processes() if "gh-capture" in command and marker in command]
        os.kill(child, signal.SIGTERM)
        for _ in range(20):
            if wrapper.log.exists():
                break
            time.sleep(0.05)
        # Stopped before what gh left running wrote again, with what came so far recorded.
        self.assertEqual(wrapper.records()[0]["stdout"], "early\n")
        for _ in range(20):  # gone, or a zombie, whose command line is empty, in a container without init
            if not any(pid == child and marker in command for pid, command in processes()):
                break
            time.sleep(0.05)
        else:
            self.fail("still running")

    def test_long_output_is_cut_short_in_the_log_only(self):
        wrapper = Wrapper(self)
        # Made by the fake: Linux takes no environment value over 128 KiB.
        result = wrapper.run("api", "repos/o/r/contents", FAKE_GH="long", FAKE_SIZE="300000")
        self.assertEqual(result.stdout, b"x" * 300000)
        [record] = wrapper.records()
        self.assertEqual((record["stdout"]["truncated"], record["stdout"]["bytes"], len(record["stdout"]["head"])),
                         (True, 300000, 256 * 1024))


def processes():
    """(pid, command line) for each process: from /proc on Linux, whose slim images lack ps."""
    if os.path.isdir("/proc/self"):
        found = []
        for name in os.listdir("/proc"):
            try:
                found.append((int(name), Path(f"/proc/{name}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")))
            except (ValueError, OSError):
                pass
        return found
    out = subprocess.run(["/bin/ps", "-A", "-o", "pid=,command="], capture_output=True, text=True).stdout
    return [(int(pid), command) for pid, _, command in (line.strip().partition(" ") for line in out.splitlines())]


def no_core():
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


class SignalTest(unittest.TestCase):
    """#197 defect 5: the wrapper dies of the signal that killed gh, and passes the caller's to gh.
    Crash-type signals are tested on Linux only: macOS shows a crash dialog for each."""

    def started(self, wrapper, mode, **env):
        process = subprocess.Popen([sys.executable, str(WRAPPER), "pr", "checks", "--watch"],
                                   env=wrapper.env | {"FAKE_GH": mode} | env, cwd=wrapper.root, preexec_fn=no_core,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        for stream in (process.stdout, process.stderr):
            self.addCleanup(stream.close)
        return process

    def test_gh_killed_by_a_signal(self):
        wrapper = Wrapper(self)
        for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP, signal.SIGKILL):
            process = self.started(wrapper, "kill", FAKE_SIGNAL=str(int(number)))
            out, errors = process.communicate(timeout=30)
            self.assertEqual((process.returncode, out, errors), (-number, b"dying\n", b""), number)
        self.assertEqual([(r["exit"], r["signal"]) for r in wrapper.records()],
                         [(-15, "SIGTERM"), (-2, "SIGINT"), (-1, "SIGHUP"), (-9, "SIGKILL")])

    @unittest.skipUnless(sys.platform.startswith("linux"), "crash-type signals only on Linux")
    def test_gh_crashing(self):
        wrapper = Wrapper(self)
        for number in (signal.SIGSEGV, signal.SIGABRT):
            process = self.started(wrapper, "kill", FAKE_SIGNAL=str(int(number)))
            process.communicate(timeout=30)
            self.assertEqual(process.returncode, -number, number)
        self.assertEqual([r["signal"] for r in wrapper.records()], ["SIGSEGV", "SIGABRT"])

    @unittest.skipUnless(hasattr(signal, "SIGRTMIN"), "real-time signals (Linux)")
    def test_a_signal_without_a_name(self):
        wrapper = Wrapper(self)
        number = signal.SIGRTMIN + 3
        process = self.started(wrapper, "kill", FAKE_SIGNAL=str(int(number)))
        process.communicate(timeout=30)
        self.assertEqual(process.returncode, -number)
        self.assertEqual(wrapper.records()[0]["signal"], f"signal {int(number)}")

    def test_the_callers_signal_reaches_gh(self):
        wrapper = Wrapper(self)
        for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            note = wrapper.root / f"note-{int(number)}"
            process = self.started(wrapper, "wait", FAKE_NOTE=str(note))
            ready, _, _ = select.select([process.stdout], [], [], 10)
            self.assertTrue(ready and process.stdout.readline() == b"ready\n", number)
            process.send_signal(number)
            process.communicate(timeout=30)
            self.assertEqual(process.returncode, -number, number)
            self.assertEqual(note.read_text(), f"{int(number)}\n", number)

    def test_a_caller_that_stops_reading(self):
        wrapper = Wrapper(self)
        process = self.started(wrapper, "flood")
        self.assertEqual(process.stdout.readline(), b"line\n")
        process.stdout.close()
        process.wait(timeout=30)
        self.assertEqual(process.returncode, -signal.SIGPIPE)


class LogFailureTest(unittest.TestCase):
    """#197 defect 6: a log that can't be written never breaks the gh call."""

    def test_a_log_path_without_a_directory(self):
        wrapper = Wrapper(self)
        result = wrapper.run("pr", "list", GH_CAPTURE_LOG="calls.jsonl")
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, b"out pr list\n", b"err\n"))
        self.assertEqual(json.loads((wrapper.root / "calls.jsonl").read_text())["argv"], ["pr", "list"])

    def test_without_redact_py_nothing_is_recorded_and_gh_still_runs(self):
        wrapper = Wrapper(self)
        alone = wrapper.root / "alone" / "gh-capture"
        alone.parent.mkdir()
        alone.write_text(WRAPPER.read_text())
        result = subprocess.run([sys.executable, str(alone), "pr", "list"], env=wrapper.env, cwd=wrapper.root,
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, b"out pr list\n", b"err\n"))
        self.assertFalse(wrapper.log.exists())

    def test_a_broken_or_older_redact_py_only_stops_the_record(self):
        wrapper = Wrapper(self)
        for source in ("def broken(:\n", "TOKEN = None\n"):
            alone = wrapper.root / "copy" / "gh-capture"
            alone.parent.mkdir(exist_ok=True)
            alone.write_text(WRAPPER.read_text())
            (alone.parent / "redact.py").write_text(source)
            result = subprocess.run([sys.executable, str(alone), "pr", "list"], env=wrapper.env, cwd=wrapper.root,
                                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, b"out pr list\n", b"err\n"), source)
            self.assertFalse(wrapper.log.exists())

    def test_a_deleted_cwd_and_closed_streams(self):
        wrapper = Wrapper(self)
        shells = {"deleted cwd": 'mkdir gone && cd gone && rmdir ../gone && exec "$0" "$@"',
                  "closed stdin": 'exec "$0" "$@" <&-', "closed stdout": 'exec "$0" "$@" >&-',
                  "closed stderr": 'exec "$0" "$@" 2>&-', "closed both": 'exec "$0" "$@" >&- 2>&-'}
        # As for gh, which writes a closed stream's output to /dev/null: nothing fails.
        expected = {"closed stdout": (0, b"", b"err\n"), "closed stderr": (0, b"out pr list --json number\n", b""),
                    "closed both": (0, b"", b"")}
        for name, shell in shells.items():
            result = subprocess.run(["/bin/sh", "-c", shell, sys.executable, str(WRAPPER), "pr", "list", "--json", "number"],
                                    env=wrapper.env, cwd=wrapper.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=30)
            self.assertEqual((result.returncode, result.stdout, result.stderr),
                             expected.get(name, (0, b"out pr list --json number\n", b"err\n")), name)
        records = wrapper.records()
        self.assertEqual([record.get("closed") for record in records],
                         [None, None, ["stdout"], ["stderr"], ["stdout", "stderr"]])
        self.assertFalse(any("forward_failed" in record for record in records))

    def test_a_closed_stdin_gh_was_told_to_read(self):
        wrapper = Wrapper(self)
        result = subprocess.run(["/bin/sh", "-c", 'exec "$0" "$@" <&-', sys.executable, str(WRAPPER), "issue", "create",
                                 "--body-file", "-"], env=wrapper.env, cwd=wrapper.root, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=30)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, b"out issue create --body-file -\n", b"err\n"))
        [record] = wrapper.records()
        self.assertNotIn("stdin", record)

    def test_an_existing_log_is_narrowed_to_its_owner(self):
        wrapper = Wrapper(self)
        wrapper.log.parent.mkdir()
        wrapper.log.write_text("")
        wrapper.log.chmod(0o644)
        wrapper.run("pr", "list")
        self.assertEqual((wrapper.log.stat().st_mode & 0o777, len(wrapper.records())), (0o600, 1))

    def test_a_log_that_isnt_a_plain_file_of_ours_is_never_written(self):
        wrapper = Wrapper(self)
        target = wrapper.root / "elsewhere"
        target.write_text("")
        target.chmod(0o644)
        wrapper.log.parent.mkdir()
        wrapper.log.symlink_to(target)
        fifo = wrapper.root / "fifo"
        os.mkfifo(fifo)
        for log in (wrapper.log, fifo):
            result = wrapper.run("pr", "list", GH_CAPTURE_LOG=str(log))
            self.assertEqual((result.returncode, result.stdout), (0, b"out pr list\n"), log)
        self.assertEqual((target.read_text(), target.stat().st_mode & 0o777), ("", 0o644))

    def test_a_log_that_cant_be_written(self):
        wrapper = Wrapper(self)
        blocker = wrapper.root / "file"
        blocker.write_text("")
        for log in (blocker / "calls.jsonl", wrapper.root):
            result = wrapper.run("pr", "list", GH_CAPTURE_LOG=str(log), FAKE_EXIT="2")
            self.assertEqual((result.returncode, result.stdout, result.stderr), (2, b"out pr list\n", b"err\n"), log)


if __name__ == "__main__":
    unittest.main()
