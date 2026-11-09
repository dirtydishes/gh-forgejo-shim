"""The replay's own judgement, on the committed captures, without a forge."""
import json
import os
import re
import unittest
from pathlib import Path

os.environ.setdefault("REPLAY_FIXTURES", str(Path(__file__).resolve().parents[2] / "fixtures/clients"))
import replay  # noqa: E402


def case(forge, stdout, exit=0, where="root"):
    return {"forge": forge, "where": where, "exit": exit, "stdout": stdout, "env": {}}


class CasesTests(unittest.TestCase):
    def test_every_document_and_call_is_replayed(self):
        calls, skipped = replay.cases()
        queries = [arg for call in calls for arg in call["forge"] if arg.startswith("query=")]
        for field in ("bodyHTML", "latestReviews", "viewerPermission", "mergeStateStatus", "includeSummary"):
            self.assertTrue(any(field in query for query in queries), field)
        aliases = {len(re.findall(r"\bp\d+: repository", query)) for query in queries}
        self.assertTrue({1, 8} <= aliases, aliases)
        commands = {" ".join(call["forge"][:2]) for call in calls}
        self.assertEqual(commands, {"--version", "api graphql", "api user", "auth status", "auth token", "pr list",
                                    "pr view"})
        self.assertEqual(sum(len(which) for which in skipped.values()), 11)

    def test_github_answers_win_over_the_forge_before_the_shim(self):
        calls, _ = replay.cases()
        token = [call for call in calls if call["forge"] == ["auth", "token", "--hostname", "forge.test"]]
        self.assertEqual([call["exit"] for call in token], [0])
        merged = [call for call in calls if call["forge"][:2] == ["pr", "list"] and "s/merge-me" in call["forge"]]
        self.assertEqual(json.loads(merged[0]["stdout"])[0]["state"], "MERGED")

    def test_the_forge_stands_in_for_github(self):
        self.assertEqual(replay.forge_argv(["api", "user", "--hostname", "github.com"]),
                         ["api", "user", "--hostname", "forge.test"])
        self.assertEqual(replay.forge_argv(["pr", "list", "--repo", "<user>/<repo>"]),
                         ["pr", "list", "--repo", "forge.test/dirtydishes/gfj-oracle"])
        self.assertEqual(replay.forge_argv(["pr", "merge", "--repo", "github.com/<user>/<repo>"]),
                         ["pr", "merge", "--repo", "forge.test/dirtydishes/gfj-oracle"])


class CompareTests(unittest.TestCase):
    def test_a_graphql_answer_needs_every_key_and_type(self):
        want = {"data": {"pullRequest": {"number": 1, "state": "OPEN", "url": "https://github.com/x", "isDraft": False}}}
        got = {"data": {"pullRequest": {"number": 1, "state": "CLOSED", "isDraft": "no", "extra": 1}}}
        problems = replay.differences(want, got, exact=True)
        self.assertEqual(problems, ["$.data.pullRequest: missing url", "$.data.pullRequest: unexpected extra",
                                    "$.data.pullRequest.state: 'CLOSED', GitHub 'OPEN'",
                                    '$.data.pullRequest.isDraft: "str", GitHub "bool"',
                                    "$.data.pullRequest.isDraft: 'no', GitHub False"])

    def test_a_rest_answer_may_lack_keys_but_not_add_them(self):
        self.assertEqual(replay.differences({"login": "a", "bio": "x"}, {"login": "a"}, exact=False), [])
        self.assertEqual(replay.differences({"login": "a"}, {"login": "a", "x": 1}, exact=False),
                         ["$: unexpected x"])

    def test_lists_and_documented_differences(self):
        checks = [{"__typename": "CheckRun", "name": "check"}]
        self.assertEqual(replay.differences({"nodes": checks}, {"nodes": []}, exact=True), ["$.nodes: 0 items, GitHub 1"])
        head = {"url": "https://github.com/x"}
        merged = replay.differences({"state": "MERGED", "headRepository": head}, {"state": "MERGED", "headRepository": None},
                                    exact=True)
        opened = replay.differences({"state": "OPEN", "headRepository": head}, {"state": "OPEN", "headRepository": None},
                                    exact=True)
        self.assertEqual((merged, len(opened)), ([], 1))

    def test_exit_delegation_and_auth_status_text(self):
        status = case(["auth", "status", "--active", "--hostname", "forge.test"],
                      "github.com\n  ✓ Logged in to github.com account <user> (keyring)\n  - Token: gho_***\n")
        good = "forge.test\n  ✓ Logged in to forge.test account dirtydishes (<run dir>/hosts.yml)\n  - Token: ***\n"
        self.assertEqual(replay.compare(status, "client", 0, good, []), [])
        self.assertEqual(len(replay.compare(status, "client", 0, good.replace("dirtydishes", "other"), [])), 1)
        self.assertEqual(replay.compare(status, "client", 1, good, ["auth status"]),
                         ["exit 1, expected 0", "reached real gh outside a launch: auth status"])
        version = case(["--version"], "gh version 2.103.3\n")
        self.assertEqual(replay.compare(version, "client", 0, "gh version 2.103.3\n", []),
                         ["expected to reach real gh, and didn't"])
        self.assertEqual(replay.compare(version, "server", 0, "gh version 2.103.3\n", []), [])
        newer = "gh version 2.110.0 (2027-01-05)\nhttps://github.com/cli/cli/releases/tag/v2.110.0\n"
        captured = case(["--version"], "gh version 2.103.3 (2026-10-26)\nhttps://github.com/cli/cli/releases/tag/v2.103.3\n")
        self.assertEqual(replay.compare(captured, "server", 0, newer, []), [])
        self.assertEqual(len(replay.compare(captured, "server", 0, "gh: unknown flag\n", [])), 1)
        token = case(["auth", "token"], {"bytes": 41, "redacted": True}, where="checkout")
        self.assertEqual(replay.compare(token, "client", 0, "secret\n", []), [])


if __name__ == "__main__":
    unittest.main()
