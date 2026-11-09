"""gh-capture against a fake gh: what it passes through, what it records, and what it never does."""

import importlib.machinery
import importlib.util
import json
import os
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
import os, signal, sys, time
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
if mode == "grandchild":
    # Something gh starts and leaves running, holding gh's output (a credential cache, a browser).
    print("bye", flush=True)
    import subprocess
    subprocess.Popen(["sleep", "8"])
if mode == "interleave":
    for line in range(1, 41):
        (sys.stdout if line % 2 else sys.stderr).write(f"{line}\n")
        (sys.stdout if line % 2 else sys.stderr).flush()
if mode == "long":
    sys.stdout.write(os.environ.get("FAKE_PREFIX", "") + "x" * int(os.environ["FAKE_SIZE"]))
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
    os.kill(os.getpid(), int(os.environ["FAKE_SIGNAL"]))
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

    def test_no_ps_and_no_proc_only_cut_the_chain_short(self):
        spec = importlib.util.spec_from_loader("gh_capture", importlib.machinery.SourceFileLoader("gh_capture", str(WRAPPER)))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with mock.patch.object(module.os.path, "exists", return_value=False), \
                mock.patch.object(module.subprocess, "run", side_effect=FileNotFoundError("/bin/ps")):
            self.assertEqual(module.process_chain(), [])


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
        self.hidden("api", "-X", "POST", "orgs/o/actions/runners/generate-jitconfig", output='{"encoded_jit_config":"e30="}')
        self.hidden("api", "repos/o/r/actions/runners/registration-token?per_page=1", output='{"token":"AABB"}')
        self.hidden("api", "user/applications/oauth2", "--jq", ".client_secret", output="gto_" + "q" * 30 + "\n")
        self.hidden("pr", "view", "--template", "{{.token}}")

    def test_credentials_in_headers_cut_short_answers_and_new_prefixes(self):
        wrapper = Wrapper(self)
        answer = 'HTTP/2.0 201 Created\r\nContent-Type: application/json\r\n\r\n{"client_id":"c1","client_secret":"gto_' + "q" * 30 + '"}'
        wrapper.run("api", "-i", "-X", "POST", "user/applications/oauth2", FAKE_GH="print", FAKE_OUT=answer)
        wrapper.run("api", "repos/o/r/contents", FAKE_GH="long", FAKE_SIZE="300000", FAKE_PREFIX='{"token": "AABBCC", ')
        first, second = wrapper.records()
        self.assertIn('"client_secret":"<redacted>"', first["stdout"])
        self.assertNotIn("gto_", first["stdout"])
        self.assertTrue(second["stdout"]["head"].startswith('{"token": "<redacted>", '))

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
        self.assertEqual(result.returncode, 0)
        self.assertEqual(wrapper.records()[0]["stdout"]["bytes"], 500000)

    def test_one_place_for_both_streams_keeps_their_order(self):
        wrapper = Wrapper(self)
        for _ in range(5):
            result = subprocess.run([sys.executable, str(WRAPPER), "pr", "checks"], env=wrapper.env | {"FAKE_GH": "interleave"},
                                    cwd=wrapper.root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, timeout=30)
            self.assertEqual(result.stdout, "".join(f"{line}\n" for line in range(1, 41)).encode())
        record = wrapper.records()[0]
        self.assertEqual((record["stderr"], record["stdout"].count("\n")), ({"merged_into_stdout": True}, 40))

    def test_something_gh_leaves_running_doesnt_hold_the_caller(self):
        wrapper = Wrapper(self)
        started = time.monotonic()
        result = wrapper.run("pr", "view", "--web", FAKE_GH="grandchild")
        self.assertLess(time.monotonic() - started, 4)
        self.assertEqual((result.returncode, result.stdout), (0, b"bye\n"))
        self.assertEqual(wrapper.records()[0]["stdout"], "bye\n")

    def test_long_output_is_cut_short_in_the_log_only(self):
        wrapper = Wrapper(self)
        # Made by the fake: Linux takes no environment value over 128 KiB.
        result = wrapper.run("api", "repos/o/r/contents", FAKE_GH="long", FAKE_SIZE="300000")
        self.assertEqual(result.stdout, b"x" * 300000)
        [record] = wrapper.records()
        self.assertEqual((record["stdout"]["truncated"], record["stdout"]["bytes"], len(record["stdout"]["head"])),
                         (True, 300000, 256 * 1024))


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
        for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            process = self.started(wrapper, "kill", FAKE_SIGNAL=str(int(number)))
            out, _ = process.communicate(timeout=30)
            self.assertEqual((process.returncode, out), (-number, b"dying\n"), number)
        self.assertEqual([(r["exit"], r["signal"]) for r in wrapper.records()],
                         [(-15, "SIGTERM"), (-2, "SIGINT"), (-1, "SIGHUP")])

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
                  "closed stdin": 'exec "$0" "$@" <&-', "closed stdout": 'exec "$0" "$@" >&-'}
        for name, shell in shells.items():
            result = subprocess.run(["/bin/sh", "-c", shell, sys.executable, str(WRAPPER), "pr", "list", "--json", "number"],
                                    env=wrapper.env, cwd=wrapper.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    timeout=30)
            self.assertEqual((result.returncode, result.stderr), (0, b"err\n"), name)
        self.assertEqual(len(wrapper.records()), 3)

    def test_a_log_that_cant_be_written(self):
        wrapper = Wrapper(self)
        blocker = wrapper.root / "file"
        blocker.write_text("")
        for log in (blocker / "calls.jsonl", wrapper.root):
            result = wrapper.run("pr", "list", GH_CAPTURE_LOG=str(log), FAKE_EXIT="2")
            self.assertEqual((result.returncode, result.stdout, result.stderr), (2, b"out pr list\n", b"err\n"), log)


if __name__ == "__main__":
    unittest.main()
