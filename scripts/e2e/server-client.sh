#!/usr/bin/env bash
# Server mode as a user sees it: real gh and Claude's REST lookups against https://forge.test, where
# nginx serves Forgejo and mounts the gateway with packaging/gateway/nginx/fga-locations.conf; then
# the same routes through the shipped Caddyfile, over HTTP.
set -euo pipefail
cp /tls/cert.pem /usr/local/share/ca-certificates/forge-test.crt
update-ca-certificates >/dev/null 2>&1
export GH_CONFIG_DIR=/tmp/gh-server GH_PROMPT_DISABLED=1
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN GH_HOST
check() { if eval "$1"; then printf 'PASS %s\n' "$2"; else printf 'FAIL %s\n' "$2" >&2; exit 1; fi; }
forgejo=http://forgejo:3000/api/v1
token=$(curl -fsS -u e2e:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"server-e2e","scopes":["all"]}' "$forgejo/users/e2e/tokens" | jq -er .sha1)
api() { curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' "$@"; }
api -d '{"name":"oracle","default_branch":"main"}' "$forgejo/user/repos" >/dev/null
mkdir /tmp/seed && cd /tmp/seed
git init -q -b main && git config user.name e2e && git config user.email e2e@example.invalid
printf 'main\n' > README.md && git add . && git commit -qm main
git checkout -qb s/pass && printf 'pass\n' > pass.txt && git add . && git commit -qm s/pass
git -c "http.extraHeader=Authorization: token $token" push -q http://forgejo:3000/e2e/oracle.git main s/pass
for attempt in $(seq 1 30); do
    api "$forgejo/repos/e2e/oracle" | jq -e '.empty == false' >/dev/null && break
    [ "$attempt" -eq 30 ] && { echo 'Forgejo still reports an empty repository' >&2; exit 1; }
    sleep 1
done
api -d '{"base":"main","head":"s/pass","title":"scenario: checks pass"}' "$forgejo/repos/e2e/oracle/pulls" >/dev/null
api -d '{"context":"ci","state":"success"}' "$forgejo/repos/e2e/oracle/statuses/$(git rev-parse s/pass)" >/dev/null

# Real gh against the forge hostname, as `gh auth login --hostname <forge>` users have it.
printf '%s\n' "$token" | gh auth login --hostname forge.test --with-token
check '[ "$(gh api --hostname forge.test user --jq .login)" = e2e ]' 'gh auth login and gh api user through the proxy'
view=$(gh pr view 1 -R forge.test/e2e/oracle --json number,headRefName,state)
check '[ "$view" = "{\"headRefName\":\"s/pass\",\"number\":1,\"state\":\"OPEN\"}" ]' 'gh pr view through the proxy'
# Captured, not piped into grep -q: under pipefail an early grep exit would fail the writer.
checks=$(gh pr checks 1 -R forge.test/e2e/oracle)
check '[ "$(printf "%s" "$checks" | cut -f1,2)" = "ci	pass" ]' 'gh pr checks through the proxy'
# Claude Code and Claude desktop look up the repository and the branch's PR over REST.
check '[ "$(api https://forge.test/api/v3/repos/e2e/oracle | jq -r .full_name)" = e2e/oracle ]' 'Claude repository lookup'
check '[ "$(api "https://forge.test/api/v3/repos/e2e/oracle/pulls?head=e2e:s/pass&state=open" | jq -r ".[0].number")" = 1 ]' 'Claude head=e2e:s/pass lookup'
# The proxy passes the client's Host through, so another host stops at the gateway.
status=$(curl -sS -k -o /dev/null -w '%{http_code}' --connect-to other.test:443:forge.test:443 \
    -H "Authorization: token $token" https://other.test/api/v3/user)
check '[ "$status" = 421 ]' 'another host gets 421 through the proxy'
# A GitHub-style PR link reaches Forgejo's page.
link=$(curl -sS -o /dev/null -w '%{http_code} %{redirect_url}' https://forge.test/e2e/oracle/pull/1)
check '[ "$link" = "301 https://forge.test/e2e/oracle/pulls/1" ]' '/pull/1 redirects to Forgejo'"'"'s /pulls/1'
landing=$(curl -sSL -o /tmp/landing.html -w '%{http_code} %{url_effective}' https://forge.test/e2e/oracle/pull/1)
check '[ "$landing" = "200 https://forge.test/e2e/oracle/pulls/1" ] && grep -q "scenario: checks pass" /tmp/landing.html' 'the redirect lands on the PR page'
check '[ "$(curl -sS -o /dev/null -w "%{http_code}" https://forge.test/)" = 200 ]' 'Forgejo'"'"'s own pages still load'
# Forgejo's own API still reaches Forgejo: only /api/v3/ and /api/graphql go to the gateway.
check '[ "$(curl -fsS https://forge.test/api/v1/version | jq -r "has(\"version\")")" = true ]' 'Forgejo'"'"'s /api/v1 still reaches Forgejo'
# $http_host keeps a non-default port, so a request for forge.test:8443 is another host.
status=$(curl -sS -o /dev/null -w '%{http_code}' --connect-to forge.test:8443:forge.test:443 \
    -H "Authorization: token $token" https://forge.test:8443/api/v3/user)
check '[ "$status" = 421 ]' 'the proxy keeps the port: forge.test:8443 gets 421'
# A path a browser would resolve to another origin never redirects (raw path, as sent).
evil='//evil.com/pull/1/x%2F..%2F..%2F..%2F..%2Fa%2Fb%2Fpull%2F1'
# The gateway's own 404, so the check proves the path reached it rather than something else.
hostile=$(curl -sS --path-as-is -o /tmp/hostile.json -w '%{http_code} %{redirect_url}' "https://forge.test$evil")
check '[ "$hostile" = "404 " ] && grep -q "\"message\":\"Not Found\"" /tmp/hostile.json' "the gateway refuses $evil through nginx ($hostile)"
# A repository named like an asset still reaches the gateway, not NPM's asset-cache regex location.
check '[ "$(curl -sS -o /dev/null -w "%{http_code}" -H "Authorization: token $token" https://forge.test/api/v3/repos/e2e/site.js)" = 404 ] && [ "$(curl -sS -H "Authorization: token $token" https://forge.test/api/v3/repos/e2e/site.js | jq -r .message)" = "Not Found" ]' '/api/v3 paths ending in .js still reach the gateway'

# The shipped Caddyfile, run with its three settings from the environment.
caddy() { curl -sS --connect-to forge.test:80:caddy:80 "$@"; }
check '[ "$(caddy -o /dev/null -w "%{http_code}" -H "Authorization: token $token" http://forge.test/api/v3/meta)" = 200 ]' 'Caddy routes /api/v3 to the gateway'
check '[ "$(caddy http://forge.test/api/v1/version | jq -r "has(\"version\")")" = true ]' 'Caddy routes /api/v1 to Forgejo'
link=$(caddy -o /dev/null -w '%{http_code} %{redirect_url}' http://forge.test/e2e/oracle/pull/1)
check '[ "$link" = "301 http://forge.test/e2e/oracle/pulls/1" ]' 'Caddy: /pull/1 redirects to /pulls/1'
hostile=$(caddy --path-as-is -o /tmp/hostile.json -w '%{http_code} %{redirect_url}' "http://forge.test$evil")
check '[ "$hostile" = "404 " ] && grep -q "\"message\":\"Not Found\"" /tmp/hostile.json' "the gateway refuses $evil through Caddy ($hostile)"
# Caddy's site block answers only its own hostname, so another host's token never gets an answer.
other=$(curl -sS -o /tmp/other.json -w '%{http_code}' --connect-to other.test:80:caddy:80 \
    -H "Authorization: token $token" http://other.test/api/v3/user)
check '! grep -q "\"login\"" /tmp/other.json' "another host gets no gateway answer through Caddy (HTTP $other)"
