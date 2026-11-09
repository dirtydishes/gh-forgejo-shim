#!/usr/bin/env python3
"""Writes contract/telemetry.json, the usage report's allowed values (docs/telemetry.md).

Usage: contract.py github-schema.graphql > contract/telemetry.json

The schema is octokit/graphql-schema's schema.graphql at the commit fixtures/github-g1.graphql names.
The `names` vocabulary, which a report's `unsupported` and `failed` counts come from, is every
field GitHub's schema gives the types below, as `graphql:<Type>.<field>`, a `graphql:<Type>.other`
for each, GitHub's REST resource names as `rest:<name>` and `rest:repos.<name>`, and `graphql:other`
and `rest:other`. A report may name nothing else, so a field or route the gateway lacks or fails is
counted by GitHub's name for it, never by text a client sent.
"""
import json
import re
import sys

TYPES = [
    "Query", "Mutation", "Repository", "PullRequest", "Issue", "User", "Organization", "Commit",
    "Ref", "PullRequestReview", "IssueComment", "CheckRun", "CheckSuite",
]

# GitHub REST resources under /repos/{owner}/{repo}/.
REPO_RESOURCES = [
    "actions", "activity", "assignees", "attestations", "autolinks", "automated-security-fixes",
    "branches", "check-runs", "check-suites", "code-scanning", "codeowners", "codespaces",
    "collaborators", "comments", "commits", "community", "compare", "contents", "contributors",
    "dependabot", "dependency-graph", "deployments", "dispatches", "environments", "events",
    "forks", "generate", "git", "hooks", "import", "installation", "interaction-limits",
    "invitations", "issues", "keys", "labels", "languages", "license", "merge-upstream", "merges",
    "milestones", "notifications", "pages", "private-vulnerability-reporting", "projects",
    "properties", "pulls", "readme", "releases", "rules", "rulesets", "secret-scanning",
    "security-advisories", "stargazers", "stats", "statuses", "subscribers", "subscription", "tags",
    "tarball", "teams", "topics", "traffic", "transfer", "vulnerability-alerts", "zipball",
]

# GitHub REST resources at the API's top level.
TOP_RESOURCES = [
    "app", "applications", "apps", "authorizations", "codes_of_conduct", "emojis", "enterprises",
    "events", "feeds", "gists", "gitignore", "installation", "issues", "licenses", "markdown",
    "marketplace_listing", "meta", "networks", "notifications", "octocat", "organizations", "orgs",
    "projects", "rate_limit", "repos", "repositories", "search", "teams", "user", "users",
    "versions", "zen",
]


def fields(schema: str) -> dict[str, list[str]]:
    found = {}
    for match in re.finditer(r"^(?:type|interface) (\w+)[^{]*\{(.*?)^\}", schema, re.S | re.M):
        name, body = match.group(1), match.group(2)
        body = re.sub(r'"""(.*?)"""', "", body, flags=re.S)
        body = re.sub(r'"[^"\n]*"', "", body)
        depth, names = 0, []
        for line in body.split("\n"):
            if depth == 0:
                field = re.match(r"^\s*(\w+)\s*[(:]", line)
                if field:
                    names.append(field.group(1))
            depth += line.count("(") - line.count(")")
        found[name] = names
    return found


def main() -> None:
    schema = open(sys.argv[1], encoding="utf-8").read()
    by_type = fields(schema)
    names = {"graphql:other", "rest:other"}
    for type_name in TYPES:
        names.add(f"graphql:{type_name}.other")
        names.update(f"graphql:{type_name}.{field}" for field in by_type[type_name])
    names.update(f"rest:repos.{name}" for name in REPO_RESOURCES)
    names.update(f"rest:{name}" for name in TOP_RESOURCES)
    contract = {
        "schema": 2,
        "os": ["macos", "linux", "other"],
        "arch": ["x86_64", "aarch64", "other"],
        "mode": ["client", "server"],
        "client": ["codex", "chatgpt", "claude-code", "claude-desktop", "cursor", "other"],
        "forge": ["forgejo", "gitea", "other"],
        "graphql_types": TYPES,
        "names": sorted(names),
    }
    json.dump(contract, sys.stdout, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
