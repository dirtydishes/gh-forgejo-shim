from __future__ import annotations

import json
import threading
import unittest
import urllib.parse
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from gh_forgejo_shim.forgejo import ForgejoClient, ForgejoError, RepoRef


@dataclass(frozen=True)
class RecordedRequest:
    method: str
    path: str
    query: dict[str, list[str]]
    headers: dict[str, str]
    body: bytes
    json_body: Any


class ForgejoFixture:
    def __init__(self) -> None:
        self.requests: list[RecordedRequest] = []
        self.status_response: Any = [{"context": "ci/unit", "state": "success"}]
        self.invalid_json_path: str | None = None
        self.error_path: str | None = None

        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                fixture.handle(self)

            def do_POST(self) -> None:  # noqa: N802
                fixture.handle(self)

            def log_message(self, format: str, *args: object) -> None:
                return

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self.server.server_address
        return f"http://{host}:{port}"

    def start(self) -> None:
        self.thread.start()

    def stop(self) -> None:
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def handle(self, handler: BaseHTTPRequestHandler) -> None:
        length = int(handler.headers.get("Content-Length", "0"))
        body = handler.rfile.read(length) if length else b""
        json_body = None
        if body:
            json_body = json.loads(body.decode("utf-8"))

        parsed = urllib.parse.urlsplit(handler.path)
        record = RecordedRequest(
            method=handler.command,
            path=parsed.path,
            query=urllib.parse.parse_qs(parsed.query, keep_blank_values=True),
            headers={key: value for key, value in handler.headers.items()},
            body=body,
            json_body=json_body,
        )
        self.requests.append(record)

        if parsed.path == self.error_path:
            self.respond(handler, 418, {"message": "short and stout"}, status_text="I'm a teapot")
            return
        if parsed.path == self.invalid_json_path:
            self.respond_raw(handler, 200, b"{not json", "application/json")
            return

        if handler.command == "GET" and parsed.path == "/api/v1/user":
            self.respond(handler, 200, {"login": "dirtydishes"})
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/space%20owner/repo%2Fname":
            self.respond(handler, 200, {"full_name": "space owner/repo/name"})
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/space%20owner/repo%2Fname/statuses/abc%2F123":
            self.respond(handler, 200, self.status_response)
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo":
            self.respond(handler, 200, {"full_name": "owner/repo", "has_issues": True})
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo/pulls":
            self.respond(handler, 200, self.pull_list())
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo/pulls/7":
            self.respond(handler, 200, {"number": 7, "title": "Existing pull"})
        elif handler.command == "POST" and parsed.path == "/api/v1/repos/owner/repo/pulls":
            self.respond(handler, 201, {"number": 8, "title": json_body["title"]})
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo/pulls/7.diff":
            self.respond_raw(handler, 200, b"diff --git a/a.txt b/a.txt\n", "text/plain")
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo/pulls/7/files":
            self.respond(handler, 200, [{"filename": "a.txt"}, "ignored"])
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo/statuses/abc123":
            self.respond(handler, 200, self.status_response)
        elif handler.command == "GET" and parsed.path == "/api/v1/repos/owner/repo/issues":
            self.respond(handler, 200, [{"number": 13, "title": "Bug"}])
        elif handler.command == "POST" and parsed.path == "/api/v1/repos/owner/repo/issues":
            self.respond(handler, 201, {"number": 14, "title": json_body["title"]})
        elif handler.command == "POST" and parsed.path == "/api/v1/repos/owner/repo/issues/14/comments":
            self.respond(handler, 201, {"body": json_body["body"]})
        else:
            self.respond(handler, 404, {"message": f"unhandled {handler.command} {parsed.path}"})

    def pull_list(self) -> list[dict[str, Any]]:
        return [
            {"number": 7, "head": {"ref": "feature/local", "label": "owner:feature/local"}},
            {"number": 8, "head": {"ref": "other", "label": "owner:other"}},
            {"number": 9, "head": {"ref": "forked", "label": "contributor:feature/local"}},
            {"not": "a pull"},
        ]

    def respond(self, handler: BaseHTTPRequestHandler, status: int, payload: Any, *, status_text: str | None = None) -> None:
        data = json.dumps(payload).encode("utf-8")
        self.respond_raw(handler, status, data, "application/json", status_text=status_text)

    def respond_raw(
        self,
        handler: BaseHTTPRequestHandler,
        status: int,
        data: bytes,
        content_type: str,
        *,
        status_text: str | None = None,
    ) -> None:
        handler.send_response(status, status_text)
        handler.send_header("Content-Type", content_type)
        handler.send_header("Content-Length", str(len(data)))
        handler.end_headers()
        handler.wfile.write(data)


class ForgejoClientHttpFixtureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = ForgejoFixture()
        self.fixture.start()
        self.client = ForgejoClient("test-token", timeout=5, base_url=self.fixture.base_url)
        self.repo = RepoRef("ignored.example", "owner", "repo")

    def tearDown(self) -> None:
        self.fixture.stop()

    def test_successful_json_gets_send_auth_and_json_accept_header(self) -> None:
        self.assertEqual(self.client.get_repo(self.repo)["full_name"], "owner/repo")
        self.assertEqual(self.client.get_pull(self.repo, 7)["number"], 7)
        self.assertEqual(self.client.get_current_user("also-ignored.example")["login"], "dirtydishes")

        self.assertEqual(
            [request.path for request in self.fixture.requests],
            ["/api/v1/repos/owner/repo", "/api/v1/repos/owner/repo/pulls/7", "/api/v1/user"],
        )
        for request in self.fixture.requests:
            self.assertEqual(request.headers["Authorization"], "token test-token")
            self.assertEqual(request.headers["Accept"], "application/json")
            self.assertEqual(request.body, b"")

    def test_json_post_flows_send_expected_payloads(self) -> None:
        pull = self.client.create_pull(
            self.repo,
            title="Ship it",
            body="Ready",
            base="main",
            head="feature/local",
            draft=True,
        )
        issue = self.client.create_issue(
            self.repo,
            title="Bug",
            body="Details",
            assignees=("octo",),
            labels=(1, 2),
            milestone=3,
        )
        comment = self.client.create_issue_comment(self.repo, 14, body="Looks good")

        self.assertEqual(pull["number"], 8)
        self.assertEqual(issue["number"], 14)
        self.assertEqual(comment["body"], "Looks good")
        self.assertEqual(
            [request.path for request in self.fixture.requests],
            [
                "/api/v1/repos/owner/repo/pulls",
                "/api/v1/repos/owner/repo/issues",
                "/api/v1/repos/owner/repo/issues/14/comments",
            ],
        )
        self.assertEqual(
            self.fixture.requests[0].json_body,
            {"title": "Ship it", "body": "Ready", "base": "main", "head": "feature/local", "draft": True},
        )
        self.assertEqual(
            self.fixture.requests[1].json_body,
            {"title": "Bug", "body": "Details", "assignees": ["octo"], "labels": [1, 2], "milestone": 3},
        )
        self.assertEqual(self.fixture.requests[2].json_body, {"body": "Looks good"})
        for request in self.fixture.requests:
            self.assertEqual(request.headers["Content-Type"], "application/json")
            self.assertEqual(request.headers["Accept"], "application/json")

    def test_text_diff_uses_plain_text_accept_header(self) -> None:
        diff_text = self.client.get_pull_diff(self.repo, 7)

        self.assertIn("diff --git", diff_text)
        self.assertEqual(self.fixture.requests[-1].path, "/api/v1/repos/owner/repo/pulls/7.diff")
        self.assertEqual(self.fixture.requests[-1].headers["Accept"], "text/plain")

    def test_pull_list_filters_realistic_forgejo_head_shapes(self) -> None:
        pulls = self.client.list_pulls(self.repo, state="open", head="feature/local")

        self.assertEqual([pull["number"] for pull in pulls], [7, 9])
        request = self.fixture.requests[-1]
        self.assertEqual(request.path, "/api/v1/repos/owner/repo/pulls")
        self.assertEqual(request.query, {"state": ["open"]})

    def test_pull_files_and_commit_status_shapes_are_normalized(self) -> None:
        self.assertEqual(self.client.list_pull_files(self.repo, 7), [{"filename": "a.txt"}])
        self.assertEqual(self.client.list_commit_statuses(self.repo, "abc123"), [{"context": "ci/unit", "state": "success"}])

        self.fixture.status_response = {"statuses": [{"context": "ci/lint", "state": "failure"}]}
        self.assertEqual(self.client.list_commit_statuses(self.repo, "abc123"), [{"context": "ci/lint", "state": "failure"}])

    def test_issue_query_parameters_are_encoded(self) -> None:
        issues = self.client.list_issues(
            self.repo,
            state="closed",
            labels=("bug", "needs triage"),
            query="bad path",
            limit=25,
        )

        self.assertEqual(issues, [{"number": 13, "title": "Bug"}])
        self.assertEqual(
            self.fixture.requests[-1].query,
            {
                "state": ["closed"],
                "type": ["issues"],
                "labels": ["bug,needs triage"],
                "q": ["bad path"],
                "limit": ["25"],
            },
        )

    def test_invalid_json_raises_forgejo_error(self) -> None:
        self.fixture.invalid_json_path = "/api/v1/repos/owner/repo"

        with self.assertRaisesRegex(ForgejoError, "invalid JSON"):
            self.client.get_repo(self.repo)

    def test_http_error_includes_status_code_and_response_body(self) -> None:
        self.fixture.error_path = "/api/v1/repos/owner/repo"

        with self.assertRaisesRegex(ForgejoError, r"HTTP 418: .*short and stout"):
            self.client.get_repo(self.repo)

    def test_path_components_are_url_quoted(self) -> None:
        quoted_repo = RepoRef("ignored.example", "space owner", "repo/name")

        repo = self.client.get_repo(quoted_repo)
        statuses = self.client.list_commit_statuses(quoted_repo, "abc/123")

        self.assertEqual(repo["full_name"], "space owner/repo/name")
        self.assertEqual(statuses, [{"context": "ci/unit", "state": "success"}])
        self.assertEqual(
            [request.path for request in self.fixture.requests],
            ["/api/v1/repos/space%20owner/repo%2Fname", "/api/v1/repos/space%20owner/repo%2Fname/statuses/abc%2F123"],
        )


if __name__ == "__main__":
    unittest.main()
