#!/usr/bin/env bash
set -euo pipefail
cp /tls/cert.pem /usr/local/share/ca-certificates/forge-test.crt
update-ca-certificates >/dev/null 2>&1
export GH_CONFIG_DIR=/tmp/gh-config GH_HOST=forge.test GH_PROMPT_DISABLED=1
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
forgejo=http://forgejo:3000/api/v1
curl -fsS -u e2e:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"g1-e2e","scopes":["all"]}' "$forgejo/users/e2e/tokens" > /tmp/token.json
token=$(jq -er .sha1 /tmp/token.json)
rm /tmp/token.json
api() { curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' "$@"; }
api -d '{"name":"oracle","default_branch":"main","private":true}' "$forgejo/user/repos" > /artifacts/forgejo-repo-created.json
mkdir /tmp/seed && cd /tmp/seed
git init -q -b main
git config user.name e2e
git config user.email e2e@example.invalid
printf 'main\n' > README.md
git add README.md && git commit -qm main
for branch in s/pass s/draft; do
    git checkout -qb "$branch" main
    printf '%s\n' "$branch" > "${branch#s/}.txt"
    git add . && git commit -qm "$branch"
done
git -c "http.extraHeader=Authorization: token $token" push -q http://forgejo:3000/e2e/oracle.git main s/pass s/draft
# Forgejo processes post-receive metadata asynchronously.
for attempt in $(seq 1 30); do
    api "$forgejo/repos/e2e/oracle" >/artifacts/forgejo-repo-pushed.json
    if jq -e '.empty == false' /artifacts/forgejo-repo-pushed.json >/dev/null; then break; fi
    if [ "$attempt" -eq 30 ]; then echo 'Forgejo still reports an empty repository after push' >&2; exit 1; fi
    sleep 1
done
api -d '{"base":"main","head":"s/pass","title":"scenario: checks pass"}' "$forgejo/repos/e2e/oracle/pulls" > /tmp/pr1.json
# Forgejo 15 derives draft from its configured WIP title prefix.
api -d '{"base":"main","head":"s/draft","title":"WIP: scenario: draft"}' "$forgejo/repos/e2e/oracle/pulls" > /tmp/pr2.json
jq -e '.draft == true' /tmp/pr2.json >/dev/null
for attempt in $(seq 1 30); do
    if curl -fsS -H "Authorization: token $token" https://forge.test/api/v3/meta >/tmp/meta.json; then break; fi
    sleep 1
done
api -D /artifacts/root.headers https://forge.test/api/v3/ >/artifacts/root.json
cp /tmp/meta.json /artifacts/meta.json
export GH_DEBUG=api
printf '%s\n' "$token" | gh auth login --hostname forge.test --with-token >/artifacts/login.txt 2>/artifacts/login.debug
gh api user >/artifacts/user.json 2>/artifacts/user.debug
gh pr view 1 -R e2e/oracle --json number,title,state,url,isDraft,headRefName,baseRefName >/artifacts/pr1.json 2>/artifacts/pr1.debug
gh pr view 2 -R e2e/oracle --json number,title,state,url,isDraft,headRefName,baseRefName >/artifacts/pr2.json 2>/artifacts/pr2.debug
unset GH_DEBUG
api https://forge.test/api/v3/repos/e2e/oracle >/artifacts/repo.json
api --get --data-urlencode 'head=e2e:s/pass' --data state=open --data per_page=1 \
    https://forge.test/api/v3/repos/e2e/oracle/pulls >/artifacts/lookup.json
api --get --data-urlencode 'head=someone-else:s/pass' --data state=all \
    https://forge.test/api/v3/repos/e2e/oracle/pulls >/artifacts/wrong-owner.json
# Close, reopen and merge the same PR to verify both state representations.
api -X PATCH -d '{"state":"closed"}' "$forgejo/repos/e2e/oracle/pulls/1" >/dev/null
gh pr view 1 -R e2e/oracle --json state >/artifacts/closed.json
api -X PATCH -d '{"state":"open"}' "$forgejo/repos/e2e/oracle/pulls/1" >/dev/null
api -d '{"Do":"merge","delete_branch_after_merge":false}' "$forgejo/repos/e2e/oracle/pulls/1/merge" >/dev/null
gh pr view 1 -R e2e/oracle --json state >/artifacts/merged.json
api --get --data-urlencode 'head=e2e:s/pass' --data state=closed \
    https://forge.test/api/v3/repos/e2e/oracle/pulls >/artifacts/merged-rest.json
curl -sS -o /artifacts/missing-token.json -w '%{http_code}' https://forge.test/api/v3/user >/artifacts/missing-token.status
curl -sS -H 'Authorization: bearer invalid-test-token' -o /artifacts/bad-token.json -w '%{http_code}' https://forge.test/api/v3/user >/artifacts/bad-token.status
curl -sS -H "Authorization: bearer $token" -o /artifacts/unknown-repo.json -w '%{http_code}' https://forge.test/api/v3/repos/e2e/missing >/artifacts/unknown-repo.status
curl -sS -H "Authorization: token $token" -o /artifacts/unknown-route.json -w '%{http_code}' https://forge.test/api/v3/unsupported >/artifacts/unknown-route.status
api -d '{"query":"{ viewer { unsupported } }"}' -o /artifacts/validation.json -w '%{http_code}' https://forge.test/api/graphql >/artifacts/validation.status
api -d '{"query":"{ repository(owner:\"e2e\",name:\"missing\") { pullRequest(number:1) { number } } }"}' -o /artifacts/not-found-graphql.json -w '%{http_code}' https://forge.test/api/graphql >/artifacts/not-found-graphql.status
api --get --data state=all --data per_page=1 -D /artifacts/page1.headers https://forge.test/api/v3/repos/e2e/oracle/pulls >/artifacts/page1.json
api --get --data state=all --data per_page=1 --data page=2 https://forge.test/api/v3/repos/e2e/oracle/pulls >/artifacts/page2.json
for auth in missing invalid; do
    headers=()
    if [ "$auth" = invalid ]; then headers=(-H 'Authorization: bearer invalid-test-token'); fi
    curl -sS "${headers[@]}" -H 'Content-Type: application/json' -d '{"query":"{ viewer { login } }"}' \
        -o "/artifacts/$auth-graphql.json" -w '%{http_code}' https://forge.test/api/graphql >"/artifacts/$auth-graphql.status"
done
gh --version | head -n 1 >/artifacts/gh-version.txt
E2E_TOKEN="$token" python3 /test/assert.py
