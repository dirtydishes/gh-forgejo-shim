# Commands and output

The shim installs a durable management command named `gh-forgejo-shim` plus a shorter daily-use
alias named `gfj`. When you opt in, it can also place a user-local `gh` wrapper in front of the real
GitHub CLI. Real GitHub repositories still delegate to the real `gh`. In allowlisted Forgejo
repositories, the pull request, issue, run and API commands that coding tools use run the real `gh`
too, against a gateway the wrapper starts for the call; the shim's own code answers the rest of a
narrow set of repository, pull request, issue and check commands. The wrapper needs `gh` 2.103.3 or
newer ([Requirements](installation.md#requirements)).

This is not full `gh` emulation. It is compatibility glue for the `gh` calls and repository probes
that GitHub-style development tools commonly use.

## Routed commands

Only commands in allowlisted Forgejo repositories are routed. Everything else delegates to the real GitHub CLI unchanged.

These run the real `gh`, against a gateway the wrapper starts for the call, with the flags the gateway serves:

```sh
gh pr view | checks | list | create | comment | edit | merge | ready | close | reopen | checkout
gh issue list | view | create | comment
gh run list | view
gh api
gh auth token | status
```

With any other flag, those commands, and these, are answered by the shim's own code:

```sh
gh pr diff | status
gh repo view
```

gh's short forms (`pr co`, `pr new`, `issue ls`, `issue new`) are routed too.

Unsupported commands and unsupported flags fail explicitly when routed, or delegate to the real `gh` when they are outside the shimmed Forgejo surface.

## Pull requests

`gh pr create` and `gh pr new` support the common flags coding tools tend to use:

```text
--title/-t
--body/-b
--body-file/-F
--base/-B
--head/-H
--repo/-R
--json
--fill
--fill-first
--fill-verbose
--web/-w
--draft/-d
```

GitHub-only metadata flags such as reviewers, labels, assignees, projects, milestones, templates, recover, and maintainer-edit controls fail with a Forgejo-specific error.

By default, `gh pr create` prints the created pull request's URL.

Pull request URLs use GitHub's `/pull/<number>` layout; the README's
[Pull request links](../README.md#pull-request-links) section explains why and has the redirect for
the forge's reverse proxy.

`gh pr list` runs the real `gh`, and the gateway applies its filters, including state, limit, head, base, author, assignee and label. `--search` takes GitHub's query syntax, and qualifiers that Forgejo has no equivalent for aren't applied.

`gh pr view` and `gh pr status` return successfully when the current branch has no open PR. For JSON output, `gh pr view --json ...` returns `{}`, and `gh pr status --json ...` returns the GitHub CLI status envelope with `"currentBranch": null`.

`gh pr diff` supports pull number or branch resolution, `--repo`, `--web`, `--name-only`, `--patch`, `--color`, and `--exclude`.

`gh pr comment` supports pull number or branch resolution, `--repo`, `--body`, `--body-file`, and `--web`.

`gh pr checkout` supports pull number or branch resolution, `--repo`, `--branch`, `--detach`, `--force`, and `--recurse-submodules`.

## Checks and statuses

When `statusCheckRollup` is requested, the shim fetches Forgejo commit statuses for the pull request head SHA and maps them into GitHub-style status context objects.

The same Forgejo status data powers:

```sh
gh pr checks
gh pr checks --json bucket,completedAt,conclusion,description,detailsUrl,link,name,startedAt,state,workflow
```

Forgejo status states are normalized into `pass`, `fail`, or `pending` buckets. When Forgejo returns repeated updates for the same context, the latest status per context is shown.

## Issues

`gh issue create` and `gh issue new` support:

```text
--title/-t
--body/-b
--body-file/-F
--repo/-R
--web/-w
--assignee/-a
--label/-l
--milestone/-m
```

Labels are resolved to Forgejo label IDs before issue creation. Milestones are passed through as numeric IDs.

`gh issue list` supports:

```text
--json
--repo/-R
--jq/-q
--template/-t
--state/-s
--limit/-L
--label/-l
--search/-S
--author/-A
--assignee/-a
--mention
--milestone/-m
--web/-w
```

`gh issue view` supports issue numbers or URLs, JSON output, repo selection, `--jq`, `--template`, `--comments`, and `--web`.

## JSON output

For `--json`, the shim emits GitHub-shaped subsets when Forgejo data is available.

Pull request fields:

```text
additions, deletions, number, title, state, isDraft, url,
headRefName, baseRefName, author, createdAt, updatedAt,
mergeable, mergeStateStatus, reviewDecision, reviewRequests,
statusCheckRollup
```

Check fields:

```text
bucket, completedAt, conclusion, description, detailsUrl, link,
name, startedAt, state, workflow
```

Issue fields:

```text
assignees, author, body, closed, closedAt,
closedByPullRequestsReferences, comments, createdAt, id, isPinned,
labels, milestone, number, projectCards, projectItems, reactionGroups,
state, stateReason, title, updatedAt, url
```

Repository fields:

```text
archivedAt, assignableUsers, codeOfConduct, contactLinks, createdAt,
defaultBranchRef, deleteBranchOnMerge, description, diskUsage, forkCount,
fundingLinks, hasDiscussionsEnabled, hasIssuesEnabled, hasProjectsEnabled,
hasWikiEnabled, homepageUrl, id, isArchived, isBlankIssuesEnabled,
isEmpty, isFork, isInOrganization, isMirror, isPrivate,
isSecurityPolicyEnabled, isTemplate, isUserConfigurationRepository,
issueTemplates, issues, labels, languages, latestRelease, licenseInfo,
mentionableUsers, mergeCommitAllowed, milestones, mirrorUrl, name,
nameWithOwner, openGraphImageUrl, owner, parent, primaryLanguage,
projects, projectsV2, pullRequestTemplates, pullRequests, pushedAt,
rebaseMergeAllowed, repositoryTopics, securityPolicyUrl,
squashMergeAllowed, sshUrl, stargazerCount, templateRepository,
updatedAt, url, usesCustomOpenGraphImage, viewerCanAdminister,
viewerDefaultCommitEmail, viewerDefaultMergeMethod, viewerHasStarred,
viewerPermission, viewerPossibleCommitEmails, viewerSubscription,
visibility, watchers
```

`gh pr status --json ...` follows the GitHub CLI status envelope:

```json
{
  "currentBranch": {
    "number": 7,
    "title": "Add Forgejo support",
    "url": "https://git.example.com/owner/repo/pull/7"
  },
  "createdBy": [],
  "needsReview": []
}
```
