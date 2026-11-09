"""redact.py and leaks.py on a made-up capture holding every kind of identifying data, and the
committed captures against both."""

import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path

import leaks
import redact

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "fixtures/clients"
FORGEJO_TOKEN = "f" * 8 + "0123456789abcdef0123456789abcdef"
SHA = "4b3e7049b6173d786b9c12f7d4d8b5355df8d3d0"
GHO = "gho_" + "Zx9" * 12
# Split so a repository-wide search for private paths finds only real ones.
MAC_HOME = "/" + "Users/octo"
# Everything the made-up capture says about its person and machine.
PRIVATE = [
    "octo-person", "secret-repo", "forge.private.dev", MAC_HOME, "203.0.113.9", "2001:db8::9",
    "octo@private.dev", GHO, FORGEJO_TOKEN, "Real Name", "Somewhere Town", "my own bio", "octo_tw",
    "1234567", "MDQ6VXNlcjEyMzQ1Njc", "avatars.githubusercontent.com", SHA, "41111899791",
    "124255789264", "2026-10-30", "-0400", "AABBCCDDEEFF00112233", "sess-value", "someone-else",
    "my status", "they/them", "en-US", "Fri, 30 Oct", "1793395470", "octo", "octo-basic",
]
USER = {
    "login": "octo-person", "id": 1234567, "node_id": "MDQ6VXNlcjEyMzQ1Njc",
    "avatar_url": "https://avatars.githubusercontent.com/u/1234567?v=4", "type": "User",
    "html_url": "https://github.com/octo-person", "name": "Real Name", "blog": "https://forge.private.dev",
    "location": "Somewhere Town", "email": "octo@private.dev", "bio": "my own bio",
    "twitter_username": "octo_tw", "followers": 7, "created_at": "2018-01-16T03:58:33Z",
}
BATCH = {"data": {"viewer": {"login": "octo-person"}, "p0": {"pullRequest": {
    "url": "https://github.com/octo-person/secret-repo/pull/1", "headRefOid": SHA,
    "author": {"login": "someone-else"},
    "commits": {"nodes": [{"commit": {"oid": SHA, "statusCheckRollup": {"contexts": {"nodes": [{
        "__typename": "CheckRun", "name": "check", "startedAt": "2026-10-30T19:02:30Z",
        "completedAt": "2026-10-30T19:02:35Z",
        "detailsUrl": "https://github.com/octo-person/secret-repo/actions/runs/41111899791/job/124255789264",
    }]}}}}]}}}}}
# Forgejo's user, GitHub GraphQL's, and git's record of who wrote a commit.
FORGEJO_USER = {"id": 3, "login": "octo-person", "full_name": "Real Name", "email": "octo@private.dev",
                "avatar_url": "https://forge.private.dev/avatars/abc", "language": "en-US", "is_admin": True,
                "location": "Somewhere Town", "website": "https://forge.private.dev", "description": "my own bio",
                "followers_count": 7}
GRAPHQL_USER = {"data": {"viewer": {"login": "octo-person", "websiteUrl": "https://forge.private.dev",
                                    "twitterUsername": "octo_tw", "pronouns": "they/them",
                                    "status": {"message": "my status"}, "bioHTML": "<p>my own bio</p>"},
                         "commit": {"author": {"name": "Real Name", "email": "octo@private.dev",
                                               "date": "2026-10-30T19:02:30Z", "user": {"login": "octo-person"}}}}}


def record(ts, argv, stdout="", stderr="", cwd="/", chain=(MAC_HOME + "/Library/Application Support/Client/client",),
           **extra):
    return {"argv": argv, "chain": list(chain), "cwd": cwd,
            "duration_ms": 10, "env": {"GH_PROMPT_DISABLED": "1"}, "env_secrets_present": ["GH_TOKEN"],
            "exit": 0, "stderr": stderr, "stdin_tty": False, "stdout": stdout, "stdout_tty": False,
            "ts": ts, **extra}


RECORDS = [
    record("2026-10-30T15:16:57-0400", ["api", "user", "--hostname", "github.com"], json.dumps(USER, separators=(",", ":"))),
    record("2026-10-30T15:17:00-0400", ["auth", "status", "--hostname", "forge.private.dev"],
           "forge.private.dev\n  ✓ Logged in to forge.private.dev account octo-person (keyring)\n"
           "  - Token: gho_************************************\n"),
    record("2026-10-30T15:17:03-0400", ["api", "-H", f"Authorization: token {FORGEJO_TOKEN}",
                                        "repos/octo-person/secret-repo/actions/runners/registration-token"],
           '{"token":"AABBCCDDEEFF00112233","expires_at":"2026-10-30T20:17:03Z"}\n',
           stderr="Set-Cookie: user_session=sess-value\n", cwd=MAC_HOME + "/Code/secret-repo-fj"),
    record("2026-10-30T15:20:00-0400", ["api", "graphql", "--hostname", "github.com", "-f",
                                        'query=query { p0: repository(owner:"octo-person",name:"secret-repo") { id } }'],
           json.dumps(BATCH, separators=(",", ":")) + "\n"),
    record("2026-10-30T15:21:00-0400", ["api", "user", "--hostname", "github.com"], json.dumps(USER, separators=(",", ":"))),
    record("2026-10-30T15:21:01-0400", ["config", "get", "oauth_token", "-h", "forge.private.dev"], GHO + "\n"),
    record("2026-10-30T15:22:00-0400", ["pr", "merge", "1", "--match-head-commit", SHA, "--repo",
                                        "https://octo:pw@forge.private.dev/octo-person/secret-repo"]),
    record("2026-10-30T15:23:00-0400", ["api", "user", "--hostname", "forge.private.dev"], json.dumps(FORGEJO_USER)),
    record("2026-10-30T15:23:30-0400", ["api", "graphql", "-f", "query=query{viewer{login}}"], json.dumps(GRAPHQL_USER)),
    # Two pages back to back, and headers before the body.
    record("2026-10-30T15:24:00-0400", ["api", "--paginate", "users/octo-person/followers"],
           json.dumps([FORGEJO_USER]) + json.dumps([USER]) + "\n"),
    record("2026-10-30T15:24:15-0400", ["issue", "list", "--search", "closed:>2026-09-30", "-q",
                                        ".[] | select(.closedAt > \"2026/10/30 15:16:57\")"],
           "abc1234 2026-10-30 15:16:57 -0400 first\nlast seen Fri, 30 Oct 2026 19:24:15 GMT\n"),
    record("2026-10-30T15:24:20-0400", ["api", "rate_limit"],
           '{"resources":{"core":{"reset":1793395470}},"t":"2026-10-30T19:24:15"}' "\n"),
    record("2026-10-30T15:24:22-0400", ["api", "graphql", "-f", "query=query{a:node(id:\"PR_kwDOAbc\"){id} b:node(id:\"PR_kwDOXyz\"){id}}"],
           '{"data":{"a":{"id":"PR_kwDOAbc"},"b":{"id":"PR_kwDOXyz"},"c":{"node_id":"PR_kwDOAbc"}}}'),
    record("2026-10-30T15:24:25-0400", ["api", "repos/o/r/contents/%2FUsers%2Focto%2Fnotes"], "logged in as octo\n",
           chain=["login -pfl octo"]),
    record("2026-10-30T15:24:30-0400", ["api", "-i", "user"],
           "HTTP/2.0 200 OK\r\nContent-Type: application/json\r\nDate: Fri, 30 Oct 2026 19:24:30 GMT\r\n"
           "X-Ratelimit-Reset: 1793395470\r\n\r\n" + json.dumps(USER)),
]
MARKS = "MARK first 15:17:00\nMARK second 15:21:00\n"
LOG = ('203.0.113.9 - - [30/Oct/2026:20:06:03 +0000] "GET /api/v3/repos/octo-person/secret-repo HTTP/1.1" 401 139 "-" "client/1"\n'
       '2001:db8::9 - octo-basic [30/Oct/2026:20:06:12 +0000] "GET /api/v3/user?access_token=AABBCCDDEEFF00112233 HTTP/1.1" 401 0 "-" "client/1"\n')
DOCUMENT = 'query { p0: repository(owner:"octo-person",name:"secret-repo") { pullRequest(number:1) { headRefOid } } }\n'
STUBS = [{"contains": ["api", "user", "forge.private.dev"], "stdout": {"login": "octo-person", "avatar_url": None}}]
NAMES = ["--name", "secret-repo=<repo>", "--name", "forge.private.dev=forge.test"]


class Capture:
    """The made-up capture written out, and its redacted copy."""

    def __init__(self):
        self.directory = tempfile.TemporaryDirectory()
        root = Path(self.directory.name)
        self.source = root / "source"
        self.out = root / "out"
        self.source.mkdir()
        (self.source / "client-1.jsonl").write_text("".join(json.dumps(r) + "\n" for r in RECORDS))
        (self.source / "client-1.marks").write_text(MARKS)
        (self.source / "client-1-requests.log").write_text(LOG)
        (self.source / "doc.graphql").write_text(DOCUMENT)
        (self.source / "client-1.stubs.json").write_text(json.dumps(STUBS))
        self.files = sorted(str(p) for p in self.source.iterdir())

    def redact(self, *extra):
        return redact.main(["--out", str(self.out), *NAMES, *extra, *self.files])

    def records(self):
        return [json.loads(line) for line in (self.out / "client-1.jsonl").read_text().splitlines()]


class RedactTest(unittest.TestCase):
    def setUp(self):
        self.capture = Capture()
        self.addCleanup(self.capture.directory.cleanup)

    def test_leaks_finds_each_kind_in_the_raw_capture(self):
        kinds = {line.rsplit(": ", 1)[1] for line in leaks.check([self.capture.source])}
        self.assertTrue({"token", "token or unredacted commit ID", "credential", "IP address", "email",
                         "home directory", "avatar URL", "host"} <= kinds, kinds)

    def test_nothing_private_survives_and_leaks_passes(self):
        self.assertEqual(self.capture.redact(), 0)
        for path in self.capture.out.iterdir():
            text = path.read_text().lower()
            for value in PRIVATE:
                self.assertNotIn(value.lower(), text, f"{value} in {path.name}")
        self.assertEqual(leaks.check([self.capture.out]), [])

    def test_a_users_profile_keeps_its_shape(self):
        self.capture.redact()
        user = json.loads(self.capture.records()[0]["stdout"])
        self.assertEqual(list(user), list(USER))
        self.assertEqual(user["login"], "<user>")
        self.assertEqual((user["id"], user["followers"]), (0, 0))
        self.assertEqual((user["name"], user["bio"], user["location"], user["blog"]), ("<redacted>",) * 4)
        self.assertEqual((user["node_id"], user["avatar_url"]), ("<node-id-1>", "<avatar-url>"))
        self.assertEqual(user["html_url"], "https://github.com/<user>")

    def test_other_users_and_commit_authors_keep_only_their_logins(self):
        self.capture.redact()
        rows = self.capture.records()
        forgejo = json.loads(rows[7]["stdout"])
        self.assertEqual(forgejo["login"], "<user>")
        self.assertEqual((forgejo["full_name"], forgejo["website"], forgejo["description"], forgejo["language"]),
                         ("<redacted>",) * 4)
        self.assertEqual((forgejo["id"], forgejo["followers_count"], forgejo["is_admin"]), (0, 0, True))
        data = json.loads(rows[8]["stdout"])["data"]
        self.assertEqual(data["viewer"], {"login": "<user>", "websiteUrl": "<redacted>", "twitterUsername": "<redacted>",
                                          "pronouns": "<redacted>", "status": {"message": "<redacted>"},
                                          "bioHTML": "<redacted>"})
        author = data["commit"]["author"]
        self.assertEqual((author["name"], author["email"], author["user"]), ("<redacted>", "<redacted>", {"login": "<user>"}))

    def test_pages_and_headers_are_read_as_json(self):
        self.capture.redact()
        rows = self.capture.records()
        pages = [value for _, _, value in redact.json_spans(rows[9]["stdout"])]
        self.assertEqual([page[0]["login"] for page in pages], ["<user>", "<user>"])
        self.assertEqual((pages[0][0]["full_name"], pages[1][0]["bio"]), ("<redacted>", "<redacted>"))
        self.assertTrue(rows[9]["stdout"].endswith("]\n"))
        head, blank, body = redact.http_parts(rows[14]["stdout"])
        self.assertEqual(head, "HTTP/2.0 200 OK\r\nContent-Type: application/json\r\nDate: <redacted>\r\n"
                               "X-Ratelimit-Reset: <redacted>")
        self.assertEqual((blank, json.loads(body)["location"]), ("\r\n\r\n", "<redacted>"))

    def test_the_capturing_account_is_user_and_bots_keep_their_names(self):
        redactor = redact.Redactor([])
        answer = {"author": {"login": "someone-else"}, "mergedBy": {"login": "github-actions"}, "by": {"login": "Copilot"}}
        redactor.learn([[{"argv": ["pr", "view", "--json", "author"], "stdout": json.dumps(answer)},
                         {"argv": ["api", "user"], "stdout": json.dumps({"login": "octo-person"})}]])
        self.assertEqual(redactor.names, {"octo-person": "<user>", "someone-else": "<user-2>"})
        self.assertEqual(redactor.text("github-actions Copilot"), "github-actions Copilot")
        redactor = redact.Redactor([])
        redactor.learn(['{"data":{"x":{"login":"someone-else"},"viewer":{"login":"octo-person"}}}'])
        self.assertEqual(redactor.names["octo-person"], "<user>")

    def test_every_token_family_and_token_header(self):
        redactor = redact.Redactor([])
        tokens = ["sk-ant-api03-" + "x" * 20, "sk-proj-" + "y" * 20, "xoxp-1234567890-abc", "AKIA" + "ABCDEFGHIJKLMNOP",
                  "glrt-" + "z" * 20, "pypi-" + "w" * 20, "eyJhbGciOiJI.eyJzdWIiOiIx.c2ln"]
        self.assertEqual(redactor.text(" ".join(tokens)), " ".join(["<token>"] * len(tokens)))
        self.assertEqual(redactor.text("PRIVATE-TOKEN: abc123\nX-Api-Key: k1"), "PRIVATE-TOKEN: <token>\nX-Api-Key: <token>")
        self.assertEqual(redactor.text("task-runner desk-lamp gho_****"), "task-runner desk-lamp gho_****")

    def test_integer_times_under_any_time_key(self):
        redactor = redact.Redactor([])
        answer = {"timestamp": 1793000000, "expires": 1793000000, "lastModified": 1793000000123, "exp": 1793000000,
                  "iat": 1793000000, "created": 1793000000, "number": 1793000000, "timeout": 30}
        redacted = redactor.value(answer)
        self.assertEqual(redacted, {"timestamp": 946684800, "expires": 946684800, "lastModified": 946684800000,
                                    "exp": 946684800, "iat": 946684800, "created": 946684800, "number": 1793000000,
                                    "timeout": 30})
        self.assertIn("date", leaks.value_findings({"expires": 1793000000}))
        self.assertEqual(leaks.value_findings({key: value for key, value in redacted.items() if key != "number"}), set())

    def test_times_in_go_git_and_date_forms(self):
        redactor = redact.Redactor([])
        cases = {
            "at 2026-10-30T15:16Z": "at 2000-01-01T00:00Z",
            "at 2026-10-30 15:16:57.123456 -0400 EDT": "at 2000-01-01 00:00:00 +0000 UTC",
            "Date:   Fri Oct 30 15:16:57 2026 -0400": "Date:   Sat Jan 1 00:00:00 2000 +0000",
            "Fri Oct  3 15:16:57 EDT 2026": "Sat Jan  1 00:00:00 UTC 2000",
            "at 2026-10-30 15:16:57.123 +0800 SGT": "at 2000-01-01 00:00:00 +0000 UTC",
            "at 2026-10-30 15:16:57 -0300 -03": "at 2000-01-01 00:00:00 +0000 UTC",
            "at 2026-10-30 15:16:57 PR 1": "at 2000-01-01 00:00:00 PR 1",
            "at 2026-10-30 15:16:57.123 +0545 +0545": "at 2000-01-01 00:00:00 +0000 UTC",
            "Fri Oct 30 15:16:57 -03 2026": "Sat Jan 1 00:00:00 UTC 2000",
            "Fri Oct 30 15:16:57 SGT 2026": "Sat Jan 1 00:00:00 UTC 2000",
            "Fri Oct 30 15:16:57 ChST 2026": "Sat Jan 1 00:00:00 UTC 2000",
            "at 2026-10-30 15:16:57.123 +1000 ChST": "at 2000-01-01 00:00:00 +0000 UTC",
            "Date: Fri, 30 Oct 2026 15:16:57 -0400": "Date: Sat, 01 Jan 2000 00:00:00 +0000",
            "Date: Thu, 5 Nov 2026 15:16:57 -0400": "Date: Sat, 1 Jan 2000 00:00:00 +0000",
            "Date: Thu, 05 Nov 2026 15:16:57 -0400 (EDT)": "Date: Sat, 01 Jan 2000 00:00:00 +0000 (UTC)",
            "2026-11-05 15:16:57 -0400 Fix login": "2000-01-01 00:00:00 +0000 Fix login",
            "Thu, 5 Nov 2026 15:16:57 -0400 (UTC-04:00)": "Sat, 1 Jan 2000 00:00:00 +0000 (UTC)",
            "Thu, 5 Nov 2026 15:16:57 -0400 (Eastern Daylight Time)": "Sat, 1 Jan 2000 00:00:00 +0000 (UTC)",
            "Thu, 5 Nov 2026 15:16:57 -0400 (HEAD -> main)": "Sat, 1 Jan 2000 00:00:00 +0000 (HEAD -> main)",
        }
        for text, expected in cases.items():
            self.assertEqual(redactor.text(text), expected)
            self.assertEqual(leaks.text_findings(expected), set(), expected)
        self.assertEqual(leaks.text_findings("Sat Jan 1 00:01:11 2000 +0000"), set())
        self.assertEqual(leaks.text_findings("2000-01-01 00:00:00 +0000 SGT"), {"time zone"})
        self.assertEqual(leaks.text_findings("2000-01-01 00:00:00 +0000 +0545"), {"time zone"})
        self.assertEqual(leaks.text_findings("2000-01-01 00:00:00 +0000 ChST"), {"time zone"})
        self.assertEqual(leaks.text_findings("Sat Jan 1 00:00:00 ChST 2000"), {"date"})
        self.assertEqual(leaks.text_findings("Thu, 5 Nov 2026 15:16:57 -0400"), {"date"})
        self.assertEqual(leaks.text_findings("Sat, 01 Jan 2000 00:00:00 +0000 (EDT)"), {"time zone"})
        self.assertEqual(leaks.text_findings("Sat, 01 Jan 2000 00:00:00 +0000 (UTC-04:00)"), {"time zone"})
        self.assertEqual(leaks.text_findings("2000-01-01 00:00:00 +0000 (HEAD -> main)"), set())
        self.assertEqual(leaks.text_findings("Fri Oct 30 15:16:57 -03 2026"), {"date"})

    def test_answers_keep_their_json_style(self):
        redactor = redact.Redactor([("octo-person", "<user>")])
        answer = {"login": "octo-person", "bio": "café", "html_url": "https://github.com/octo-person?a=1&b=2"}
        for original in (json.dumps(answer, indent=2), json.dumps(answer, separators=(",", ":")),
                         json.dumps(answer, ensure_ascii=True), json.dumps(answer, separators=(",", ":"))
                         .replace("&", "\\u0026")):
            redacted = redactor.structured(original)
            expected = original.replace("octo-person", "<user>").replace("café", "<redacted>").replace("caf\\u00e9", "<redacted>")
            if "\\u0026" in original:
                expected = expected.replace("<user>", "\\u003cuser\\u003e").replace("<redacted>", "\\u003credacted\\u003e")
            self.assertEqual(redacted, expected)

    def test_hex_folder_names_are_hidden(self):
        redactor = redact.Redactor([])
        self.assertEqual(redactor.text("/Claude/claude-code/2.1.309/5af865b9901c/claude.app /runs/12345678/x"),
                         "/Claude/claude-code/2.1.309/<id>/claude.app /runs/12345678/x")
        sha = "0" * 39 + "1"
        self.assertEqual(redactor.text(f"/commits/{sha}/status"), f"/commits/{sha}/status")

    def test_versions_and_workflow_commands_are_left_alone(self):
        redactor = redact.Redactor([])
        kept = ("-H X-GitHub-Api-Version: 2022-11-28 X-Github-Api-Version-Selected: 2022-11-28 "
                "git@github.com:octo/homebox.git mail me@private.dev.")
        self.assertEqual(redactor.text(kept), kept.replace("me@private.dev", "<email>"))
        self.assertNotIn("date", leaks.text_findings("X-GitHub-Api-Version: 2022-11-28"))
        text = "npx ripgrep@14.1.0 && echo ::add-mask::$X from fe80::1:2"
        self.assertEqual(redactor.text(text), "npx ripgrep@14.1.0 && echo ::add-mask::$X from <ip>")

    def test_a_person_without_a_login_and_json_that_doesnt_parse(self):
        redactor = redact.Redactor([("octo-person", "<user>")])
        answer = {"data": {"viewer": {"name": "Real Name", "bio": "my own bio", "location": "Somewhere Town",
                                      "socialAccounts": {"nodes": [{"provider": "X", "url": "https://x.example/octo_tw"}]}},
                           "repository": {"name": "secret-repo"}}}
        self.assertEqual(redactor.value(answer)["data"], {
            "viewer": {"name": "<redacted>", "bio": "<redacted>", "location": "<redacted>",
                       "socialAccounts": {"nodes": [{"provider": "<redacted>", "url": "<redacted>"}]}},
            "repository": {"name": "secret-repo"}})
        cut = '{"login":"octo-person","name":"Real Name","bio": "my own bio"}\nwarning: rate limited\n'
        self.assertEqual(redactor.structured(cut), '{"login":"<user>","name":"<redacted>","bio": "<redacted>"}\nwarning: rate limited\n')
        self.assertEqual(redactor.structured('\ufeff{"login":"octo-person","bio":"b"}'), '\ufeff{"login":"<user>","bio":"<redacted>"}')
        learner = redact.Redactor([])
        learner.learn(['{"login":"someone-else","bio":"b"', '{"login":"octo-person"}'])
        self.assertIn("someone-else", learner.names)

    def test_a_signed_commits_payload_and_signature(self):
        redactor = redact.Redactor([])
        payload = ("tree abc\nauthor Real Name <octo@private.dev> 1793000000 -0500\n"
                   "committer Real Name <octo@private.dev> 1793000000 -0500\n\nmessage\n")
        signature = "-----BEGIN SSH SIGNATURE-----\nU1NIU0lHAAAAAQ\n-----END SSH SIGNATURE-----\n"
        self.assertEqual(redactor.text(payload), "tree abc\nauthor <redacted> <email> 946684800 +0000\n"
                                                 "committer <redacted> <email> 946684800 +0000\n\nmessage\n")
        self.assertEqual(redactor.text(signature), "-----BEGIN SSH SIGNATURE-----<redacted>-----END SSH SIGNATURE-----\n")
        answer = redactor.value({"commit": {"verification": {"verified": True, "payload": payload, "signature": signature}},
                                 "signature": {"keyId": "3AA5C34371567BD2", "isValid": True, "signer": {"login": "octo"}}})
        self.assertEqual(answer, {"commit": {"verification": {"verified": True, "payload": "<redacted>", "signature": "<redacted>"}},
                                  "signature": {"keyId": "<redacted>", "isValid": True, "signer": {"login": "<redacted>"}}})
        self.assertEqual(leaks.value_findings(answer), set())
        kinds = leaks.value_findings({"payload": payload, "signature": signature})
        self.assertTrue({"git identity", "signature"} <= kinds, kinds)

    def test_lists_and_dates_under_a_person(self):
        redactor = redact.Redactor([("octo-person", "<user>")])
        self.assertEqual(redactor.value({"login": "octo-person", "aliases": ["Real Name", "Somewhere Town", 7]}),
                         {"login": "<user>", "aliases": ["<redacted>", "<redacted>", 0]})
        self.assertEqual(redactor.value({"viewer": {"extra": ["Real Name"]}}), {"viewer": {"extra": ["<redacted>"]}})
        actor = redactor.value({"commit": {"author": {"name": "Real Name", "email": "a@private.dev", "date": "2026-10-30T15:16:57Z"}}})
        self.assertEqual(actor["commit"]["author"], {"name": "<redacted>", "email": "<redacted>", "date": "2000-01-01T00:00:00Z"})

    def test_placeholders_never_collide(self):
        redactor = redact.Redactor([("bob", "<user-2>")])
        redactor.learn([{"data": {"viewer": {"login": "octo-person"}, "x": {"login": "someone-else"}}}])
        self.assertEqual(redactor.names, {"bob": "<user-2>", "octo-person": "<user>", "someone-else": "<user-3>"})

    def test_the_viewer_from_a_bare_login(self):
        redactor = redact.Redactor([])
        redactor.learn([[{"argv": ["api", "user", "--jq", ".login"], "exit": 0, "stdout": "octo-person\n"},
                         {"argv": ["pr", "list", "--author", "octo-person"], "stdout": "[]"}]])
        self.assertEqual(redactor.names, {"octo-person": "<user>"})
        redactor = redact.Redactor([])
        redactor.learn([[{"argv": ["api", "user", "--jq", ".login"], "exit": 0, "stdout": "octo.per_son\n"}]])
        self.assertEqual(redactor.names, {"octo.per_son": "<user>"})
        for exit, stdout in ((0, "null\n"), (1, '{"message":"Bad credentials"}\n'), (0, "")):
            redactor = redact.Redactor([])
            redactor.learn([[{"argv": ["api", "user", "-q", ".login"], "exit": exit, "stdout": stdout}]])
            self.assertEqual(redactor.names, {}, stdout)

    def test_refuses_without_a_capturing_account(self):
        (self.capture.source / "client-1.jsonl").write_text(json.dumps(record(
            "2026-10-30T15:16:57-0400", ["auth", "token"], {"bytes": 41, "redacted": True})) + "\n")
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()) as stderr:
            self.capture.redact()
        self.assertIn("found no capturing account", stderr.getvalue())
        self.assertEqual(self.capture.redact("--name", "octo-person=<user>"), 0)

    def test_a_gravatar_id_is_hidden_unless_empty(self):
        redactor = redact.Redactor([])
        hashed = redactor.value({"login": "octo-person", "gravatar_id": "0bc83cb571cd1c50ba6f3e8a78ef1346"})
        self.assertEqual(hashed["gravatar_id"], "<redacted>")
        self.assertEqual(redactor.value({"login": "octo-person", "gravatar_id": ""})["gravatar_id"], "")

    def test_forgejo_usernames_are_learned(self):
        redactor = redact.Redactor([("octo-person", "<user>")])
        pusher = {"pusher": {"name": "Real Name", "email": "a@private.dev", "username": "someone-else"}}
        redactor.learn([pusher])
        self.assertEqual(redactor.names["someone-else"], "<user-2>")
        self.assertEqual(redactor.value(pusher)["pusher"],
                         {"name": "<redacted>", "email": "<redacted>", "username": "<user-2>"})
        self.assertEqual(redactor.text("https://forge.test/someone-else/x"), "https://forge.test/<user-2>/x")

    def test_logins_that_are_path_words_are_kept(self):
        redactor = redact.Redactor([])
        redactor.learn([{"data": {"viewer": {"login": "octo-person"}, "a": {"owner": {"login": "actions"}},
                                  "b": {"owner": {"login": "github"}}}}])
        self.assertEqual(redactor.names, {"octo-person": "<user>"})
        self.assertEqual(redactor.kept, ["actions", "github"])
        url = "https://github.com/octo-person/homebox/actions/runs/123"
        self.assertEqual(redactor.text(url), "https://github.com/<user>/homebox/actions/runs/1")

    def test_node_ids_are_numbered(self):
        self.capture.redact()
        row = self.capture.records()[12]
        data = json.loads(row["stdout"])["data"]
        self.assertEqual((data["a"]["id"], data["b"]["id"], data["c"]["node_id"]), ("<node-id-2>", "<node-id-3>", "<node-id-2>"))
        self.assertEqual(row["argv"][-1], 'query=query{a:node(id:"<node-id-2>"){id} b:node(id:"<node-id-3>"){id}}')

    def test_the_local_user_name_goes_wherever_it_appears(self):
        self.capture.redact()
        row = self.capture.records()[13]
        self.assertEqual((row["argv"][1], row["stdout"], row["chain"]),
                         ("repos/o/r/contents/<home>%2Fnotes", "logged in as <local-user>\n", ["login -pfl <local-user>"]))
        self.assertEqual(self.capture.records()[0]["chain"], ["<home>/Library/Application Support/Client/client"])

    def test_every_kind_of_time_and_date(self):
        self.capture.redact()
        row, rate = self.capture.records()[10:12]
        self.assertEqual(row["argv"][3], "closed:>2000-01-01")
        self.assertRegex(row["argv"][5], r'"2000/01/01 00:00:\d\d"')
        lines = row["stdout"].splitlines()
        self.assertRegex(lines[0], r"^abc1234 2000-01-01 00:00:\d\d \+0000 first$")
        self.assertRegex(lines[1], r"^last seen Sat, 01 Jan 2000 00:00:\d\d GMT$")
        answer = json.loads(rate["stdout"])
        self.assertEqual(answer["resources"]["core"]["reset"], 946684800)
        self.assertRegex(answer["t"], r"^2000-01-01T00:00:\d\d$")

    def test_times_become_offsets_and_marks_labels(self):
        self.capture.redact()
        rows = self.capture.records()
        self.assertEqual([row["t"] for row in rows], [0, 3, 6, 183, 243, 244, 303, 363, 393, 423, 438, 443, 445, 448, 453])
        self.assertEqual([row.get("mark") for row in rows], [None] + ["first"] * 3 + ["second"] * 11)
        self.assertFalse(any("ts" in row for row in rows))
        log = (self.capture.out / "client-1-requests.log").read_text().splitlines()
        self.assertEqual([line.split(" ", 4)[3] for line in log], ["[+0s]", "[+9s]"])
        self.assertTrue(all(line.startswith("<ip> ") for line in log))
        self.assertEqual([line.split(" ", 3)[1:3] for line in log], [["-", "-"], ["-", "<redacted>"]])

    def test_placeholders_are_stable_across_files_and_keep_order(self):
        self.capture.redact()
        rows = self.capture.records()
        batch = json.loads(rows[3]["stdout"])["data"]["p0"]["pullRequest"]
        sha = "0" * 39 + "1"
        self.assertEqual(batch["headRefOid"], sha)
        self.assertEqual(rows[6]["argv"][4], sha)
        self.assertEqual(batch["author"]["login"], "<user-2>")
        check = batch["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"][0]
        self.assertLess(check["startedAt"], check["completedAt"])
        self.assertEqual(check["detailsUrl"], "https://github.com/<user>/<repo>/actions/runs/1/job/1")
        self.assertIn('owner:"<user>",name:"<repo>"', (self.capture.out / "doc.graphql").read_text())
        stubs = json.loads((self.capture.out / "client-1.stubs.json").read_text())
        self.assertEqual(stubs[0]["contains"][2], "forge.test")

    def test_credentials_are_hidden(self):
        self.capture.redact()
        rows = self.capture.records()
        self.assertEqual(rows[2]["argv"][2], "Authorization: token <token>")
        self.assertEqual(json.loads(rows[2]["stdout"])["token"], "<redacted>")
        self.assertEqual(rows[2]["stderr"], "Set-Cookie: <redacted>\n")
        self.assertEqual(rows[5]["stdout"], "<token>\n")
        self.assertEqual(rows[6]["argv"][-1], "https://<redacted>@forge.test/<user>/<repo>")
        self.assertIn("Token: gho_****", rows[1]["stdout"])
        self.assertIn("access_token=<redacted>", (self.capture.out / "client-1-requests.log").read_text())

    def test_a_repeated_answer_is_stored_once(self):
        self.capture.redact()
        rows = self.capture.records()
        self.assertEqual(rows[4]["stdout"], {"same_as": 0})

    def test_screenshots_are_refused(self):
        (self.capture.source / "shot.png").write_bytes(b"\x89PNG\r\n")
        self.capture.files.append(str(self.capture.source / "shot.png"))
        with self.assertRaises(SystemExit), contextlib.redirect_stderr(io.StringIO()) as stderr:
            self.capture.redact()
        self.assertIn("refusing files it can't redact", stderr.getvalue())
        self.assertFalse(self.capture.out.exists())


class LeaksTest(unittest.TestCase):
    def kinds(self, text, suffix=".log"):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / ("capture" + suffix)
            path.write_text(text)
            findings = leaks.check([path])
        for finding in findings:
            self.assertNotIn(text.strip(), finding)
        return {finding.rsplit(": ", 1)[1] for finding in findings}

    def test_each_kind(self):
        cases = {
            "ghp_" + "a1" * 18: "token", "eyJhbGciOiJI.eyJzdWIiOiIx.sig": "token", "sk-ant-api03-" + "x" * 20: "token",
            "xoxb-1234567890-abc": "token", "AKIA" + "ABCDEFGHIJKLMNOP": "token", "npm_" + "x" * 30: "token",
            "PRIVATE-TOKEN: abc123": "credential",
            "a" * 40: "token or unredacted commit ID", "Authorization: bearer abc": "credential",
            "Cookie: a=b": "credential", "password=hunter2": "credential", "me@private.dev": "email",
            "/home/octo/x": "home directory", "C:\\Users\\octo": "home directory", "%2Fhome%2Focto": "home directory", "198.51.100.7": "IP address",
            "2001:db8::1": "IP address", "https://forge.test/avatars/abc": "avatar URL",
            "https://avatars.githubusercontent.com/u/1": "avatar URL", "https://me:pw@forge.test/": "credential",
            "https://forge.private.dev/x": "host", "git.example.ts.net": "host", "box.local": "host",
            "--repo forge.private.dev/o/r": "host", "forge.octo.sh": "host", "--hostname homebox": "host",
            "git@homebox:o/r.git": "host", "Logged in to homebox account x (keyring)": "host",
            "GH_HOST=homebox": "host", "closed:>2026-09-30": "date", "2026/10/30 15:16:57": "date",
            "Fri, 30 Oct 2026 19:24:15 GMT": "date", "[30/Oct/2026:20:06:03 +0000]": "date",
            "2026-10-30T15:16:57Z": "date", "created 2026-10-30T15:16:57.123+02:00": "date",
            "Fri Oct 30 15:16:57 2026 -0400": "date", "Fri Oct  3 15:16:57 EDT 2026": "date",
        }
        for text, kind in cases.items():
            self.assertIn(kind, self.kinds(text), text)

    def test_placeholders_and_ordinary_text_pass(self):
        ordinary = [
            "<user> <repo> <home>/Code <ip> <email> <token> <avatar-url> forge.test github.com",
            "https://api.github.com/users/<user> https://<redacted>@forge.test/x",
            "Authorization: token <token>", "Cookie: <redacted>", "0" * 39 + "1 2000-01-01T00:00:00Z",
            "closed:>2000-01-01 2000/01/01 00:00:01 Sat, 01 Jan 2000 00:00:00 GMT gh version 2.103.3 (2026-10-26)",
            "gh version 2.103.3 Versions/156.0.8078.48/Helpers /Applications/ChatGPT.app/Contents 19:02:38",
            "Codex (Service).app <home>/.claude/settings.local.json git@github.com:o/r.git",
            "ci / check (pull_request) e2e@example.invalid notes/nopr.txt npx ripgrep@14.1.0 ::add-mask::x",
        ]
        self.assertEqual(self.kinds("\n".join(ordinary) + "\n"), set())

    def test_file_and_folder_names(self):
        with tempfile.TemporaryDirectory() as directory:
            folder = Path(directory) / "homebox.local"
            folder.mkdir()
            (folder / "capture-octo@private.dev.log").write_text("198.51.100.7\n")
            (Path(directory) / "clean.jsonl").write_text("")
            findings = leaks.check([directory])
        self.assertEqual(sorted(f.rsplit(": ", 1)[1] for f in findings),
                         ["IP address", "email in the file name", "host in the file name"])
        for finding in findings:
            self.assertNotIn("homebox", finding)
            self.assertNotIn("private.dev", finding)
            self.assertIn("/<file 2 of 2>:", finding)

    def test_hosts_in_a_records_argv_and_env(self):
        for argv in (["auth", "status", "-h", "homebox"], ["api", "user", "--hostname=homebox"],
                     ["pr", "list", "-R", "homebox/o/r"], ["pr", "list", "-Rhomebox/o/r"]):
            self.assertIn("host", self.kinds(json.dumps({"argv": argv}) + "\n", ".jsonl"), argv)
        self.assertIn("host", self.kinds(json.dumps({"env": {"GH_HOST": "homebox"}}) + "\n", ".jsonl"))
        line = json.dumps({"argv": ["pr", "list", "--repo", "<user>/<repo>", "--hostname", "forge.test"],
                           "env": {"GH_HOST": "github.com"}})
        self.assertEqual(self.kinds(line + "\n", ".jsonl"), set())

    def test_a_sensitive_json_key_needs_a_placeholder(self):
        line = json.dumps({"stdout": json.dumps({"token": "AABB", "other": "x"})})
        self.assertIn("credential", self.kinds(line + "\n", ".jsonl"))
        line = json.dumps({"stdout": json.dumps({"token": "<redacted>"})})
        self.assertEqual(self.kinds(line + "\n", ".jsonl"), set())


class CommittedCapturesTest(unittest.TestCase):
    def test_nothing_identifying(self):
        self.assertEqual(leaks.check([FIXTURES]), [])

    def test_each_document_is_a_captured_query_with_its_flags(self):
        records = [json.loads(line) for path in FIXTURES.glob("*.jsonl") for line in path.read_text().splitlines()]
        documents = sorted((FIXTURES / "chatgpt-22943-documents").glob("*.graphql"))
        self.assertEqual(len(documents), 7)
        for document in documents:
            query = "query=" + document.read_text()
            flags = document.with_suffix(".vars").read_text().split()
            sent = [r["argv"] for r in records if any(a in (query, query.rstrip("\n")) for a in r["argv"])]
            self.assertTrue(sent, f"{document.name} was never sent")
            pairs = [" ".join(argv[i:i + 2]) for argv in sent for i in range(len(argv) - 1)]
            for flag, value in zip(flags[::2], flags[1::2]):
                self.assertIn(f"{flag} {value}", pairs, f"{document.name}: {flag} {value}")

    def test_redaction_was_applied(self):
        for path in FIXTURES.glob("*.jsonl"):
            for line in path.read_text().splitlines():
                row = json.loads(line)
                self.assertIn("t", row)
                self.assertNotIn("ts", row)
                self.assertFalse(leaks.HOME.search(row["cwd"] + "".join(row["chain"])))


if __name__ == "__main__":
    unittest.main()
