#!/usr/bin/env bash
# The gh commands coding agents run on a pull request, and those ChatGPT desktop's PR-code tab runs,
# with their exact argv, through nginx and the gateway (MAPPING §11). Run by server-client.sh, which
# has logged gh in to forge.test and left the seed checkout in /tmp/seed; `token` is e2e's.
set -euo pipefail
check() { if eval "$1"; then printf 'PASS %s\n' "$2"; else printf 'FAIL %s\n' "$2" >&2; exit 1; fi; }
forgejo=http://forgejo:3000/api/v1
api() { curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' "$@"; }

# A reviewer, and a PR of three commits across four files with an inline comment on
# src/cli/commands.ts line 45 and one on the old side of README.md line 3.
api -d '{"username":"reviewer","email":"reviewer@example.invalid","password":"e2e-disposable-password","must_change_password":false}' \
    "$forgejo/admin/users" >/dev/null
reviewer=$(curl -fsS -u reviewer:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"agents-e2e","scopes":["all"]}' "$forgejo/users/reviewer/tokens" | jq -er .sha1)
api -X PUT -d '{"permission":"write"}' "$forgejo/repos/e2e/oracle/collaborators/reviewer" >/dev/null
cd /tmp/seed
git checkout -q main
mkdir -p src/cli docs
printf '# demo\n\nA small CLI.\n' > README.md
printf '*.ts text eol=lf\n' > .gitattributes
printf '*.snap binary\n' > src/.gitattributes
for i in $(seq 1 80); do printf 'export const line%d = %d;\n' "$i" "$i"; done > src/cli/commands.ts
printf '# Guide\n\nOld guide.\n' > docs/guide.md
git add . && git commit -qm 'initial app'
git checkout -qb agents
for i in $(seq 1 80); do
    if [ "$i" -ge 40 ] && [ "$i" -le 50 ]; then printf 'export const line%d = %d;\n' "$i" $((i * 10)); else printf 'export const line%d = %d;\n' "$i" "$i"; fi
done > src/cli/commands.ts
git commit -qam 'commands: scale lines 40 to 50'
printf 'export const twice = (n: number) => n * 2;\n' > src/util.ts
git add src/util.ts && git commit -qm 'util: add twice'
git rm -q docs/guide.md && printf '# demo\n\nA small CLI with helpers.\n' > README.md
git commit -qam 'docs: drop the old guide' -m 'The README covers it now.'
git -c "http.extraHeader=Authorization: token $token" push -q http://forgejo:3000/e2e/oracle.git main agents
base=$(git rev-parse main) head=$(git rev-parse agents)
n=$(api -d '{"base":"main","head":"agents","title":"Scale the commands and add helpers","body":"Scales lines 40 to 50."}' \
    "$forgejo/repos/e2e/oracle/pulls" | jq -er .number)
for attempt in $(seq 1 30); do
    [ "$(api "$forgejo/repos/e2e/oracle/pulls/$n" | jq -r .mergeable)" = true ] && break
    [ "$attempt" -eq 30 ] && { echo "Forgejo never finished checking PR $n" >&2; exit 1; }
    sleep 1
done
curl -fsS -H "Authorization: token $reviewer" -H 'Content-Type: application/json' -d "{\"body\":\"A couple of questions.\",\"event\":\"COMMENT\",\"commit_id\":\"$head\",
    \"comments\":[{\"path\":\"src/cli/commands.ts\",\"body\":\"Why scale line 45?\",\"new_position\":45},
                  {\"path\":\"README.md\",\"body\":\"The old wording was clearer.\",\"old_position\":3}]}" \
    "$forgejo/repos/e2e/oracle/pulls/$n/reviews" >/dev/null
api -d '{"body":"Thanks for the review."}' "$forgejo/repos/e2e/oracle/issues/$n/comments" >/dev/null
cd /tmp
R=forge.test/e2e/oracle

# Codex in ChatGPT desktop, as it ran gh itself.
view=$(gh pr view "$n" -R "$R" --json number,title,url,body,reviews,comments,files)
check '[ "$(printf "%s" "$view" | jq -c "[.files[] | [.path, .changeType, .additions, .deletions]]")" = "[[\"README.md\",\"MODIFIED\",1,1],[\"docs/guide.md\",\"DELETED\",0,3],[\"src/cli/commands.ts\",\"MODIFIED\",11,11],[\"src/util.ts\",\"ADDED\",1,0]]" ] && [ "$(printf "%s" "$view" | jq -r ".reviews[0].state, .comments[0].body")" = "COMMENTED
Thanks for the review." ]' 'gh pr view --json number,title,url,body,reviews,comments,files'
view=$(gh pr view "$n" -R "$R" --json title,body,baseRefName,headRefName,headRefOid,files,url)
check '[ "$(printf "%s" "$view" | jq -r ".headRefOid, (.files | length)")" = "$head
4" ]' 'gh pr view --json title,body,baseRefName,headRefName,headRefOid,files,url'
comment='[.[0] | .path, .line, .side, .commit_id, .user.login, .body] | join(" ")'
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/pulls/$n/comments" --jq "$comment")" = "src/cli/commands.ts 45 RIGHT $head reviewer Why scale line 45?" ]' 'gh api --hostname H repos/O/R/pulls/N/comments'
check '[ "$(GH_HOST=forge.test gh api "repos/e2e/oracle/pulls/$n/comments" --jq ".[1] | [.path, .line, .side] | join(\" \")")" = "README.md 3 LEFT" ]' 'gh api repos/O/R/pulls/N/comments (GH_HOST)'
threads=$(gh api graphql --hostname forge.test -f query="query{repository(owner:\"e2e\",name:\"oracle\"){pullRequest(number:$n){ reviewThreads(first:100){ nodes{ isResolved comments(first:100){ nodes{ body path line url } } } } }}}" \
    --jq '[.data.repository.pullRequest.reviewThreads.nodes[] | .comments.nodes[0] | "\(.path):\(.line)"] | join(" ")')
check '[ "$threads" = "src/cli/commands.ts:45 README.md:3" ]' 'reviewThreads comments { path line url }'

# ChatGPT desktop's PR-code tab.
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/pulls/$n" --jq ".base.sha, .head.sha")" = "$base
$head" ]' 'gh api --hostname H repos/O/R/pulls/N: base.sha and head.sha'
compare=$(gh api --hostname forge.test "repos/e2e/oracle/compare/$base...$head?per_page=100" --paginate --slurp)
check '[ "$(printf "%s" "$compare" | jq -r ".[0].merge_base_commit.sha, (.[0].commits | map(.sha) | join(\",\")), .[0].commits[0].parents[0].sha, .[0].commits[2].commit.message, (.[0].files | length)")" = "$base
$(git -C /tmp/seed log --reverse --format=%H main..agents | paste -sd, -)
$base
docs: drop the old guide

The README covers it now.
4" ]' 'gh api compare/BASE...HEAD --paginate --slurp: merge base, commits, files'
gh api --hostname forge.test "repos/e2e/oracle/compare/$base...$head?per_page=100" -H "Accept: application/vnd.github.diff" > /tmp/compare.diff
gh pr diff "$n" -R "$R" > /tmp/pr.diff
check 'cmp -s /tmp/compare.diff /tmp/pr.diff && grep -q "^+export const line45 = 450;" /tmp/pr.diff' 'gh api compare/BASE...HEAD as application/vnd.github.diff is the PR diff'
content=$(gh api --hostname forge.test "repos/e2e/oracle/contents/src/cli/commands.ts?ref=$head" -H "Accept: application/vnd.github.object+json" --jq .content)
check 'printf "%s" "$content" | base64 -d | grep -qx "export const line45 = 450;"' 'gh api contents/PATH?ref=SHA as object+json: base64'
blobs=$(gh api graphql --hostname forge.test -f query="query{repository(owner:\"e2e\",name:\"oracle\"){f0: object(expression:\"$head:.gitattributes\"){... on Blob{text isTruncated}} f1: object(expression:\"$head:src/.gitattributes\"){... on Blob{text isTruncated}} f2: object(expression:\"$head:src/cli/.gitattributes\"){... on Blob{text isTruncated}}}}" --jq .data.repository | jq -cS .)
check '[ "$blobs" = "{\"f0\":{\"isTruncated\":false,\"text\":\"*.ts text eol=lf\\n\"},\"f1\":{\"isTruncated\":false,\"text\":\"*.snap binary\\n\"},\"f2\":null}" ]' 'object(expression:"SHA:dir/.gitattributes"): text, and null for a missing path'
# The same comparison and commit by branch name, with a slash.
git -C /tmp/seed -c "http.extraHeader=Authorization: token $token" push -q http://forgejo:3000/e2e/oracle.git agents:refs/heads/feature/agents
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/compare/main...feature/agents" --jq ".merge_base_commit.sha, .commits[-1].sha, .ahead_by, (.files | length), .url")" = "$base
$head
3
4
https://forge.test/api/v3/repos/e2e/oracle/compare/main...feature/agents" ]' 'gh api compare/main...feature/agents: a branch with a slash'
check '[ "$(gh api graphql --hostname forge.test -f query="query{repository(owner:\"e2e\",name:\"oracle\"){c: object(expression:\"feature/agents\"){oid} b: object(expression:\"feature/agents:.gitattributes\"){... on Blob{text}}}}" --jq ".data.repository | .c.oid, .b.text")" = "$head
*.ts text eol=lf" ]' 'object(expression:"feature/agents") and "feature/agents:PATH"'

# Other reads agents run on a PR.
check '[ "$(gh pr diff "$n" -R "$R" --patch | head -1)" = "From $(git -C /tmp/seed rev-parse agents~2) Mon Sep 17 00:00:00 2001" ]' 'gh pr diff --patch prints a patch'
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/pulls/$n/files" --jq ".[] | select(.filename == \"src/util.ts\") | .status, .patch")" = "added
@@ -0,0 +1 @@
+export const twice = (n: number) => n * 2;" ]' 'gh api repos/O/R/pulls/N/files'
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/pulls/$n/reviews" --jq ".[0] | .state, .user.login")" = "COMMENTED
reviewer" ]' 'gh api repos/O/R/pulls/N/reviews'
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/issues/$n/comments" --jq ".[0].body")" = "Thanks for the review." ]' 'gh api repos/O/R/issues/N/comments'
check '[ "$(gh api --hostname forge.test "repos/e2e/oracle/pulls/$n/commits" --jq ".[-1].sha")" = "$head" ]' 'gh api repos/O/R/pulls/N/commits, oldest first'
