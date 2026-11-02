#!/usr/bin/env bash
set -euo pipefail
unset GH_DEBUG GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
export GH_CONFIG_DIR=/tmp/gh-g2 GH_HOST=forge.test GH_PROMPT_DISABLED=1
curl -fsS -u dirtydishes:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"g2-e2e","scopes":["all"]}' http://forgejo:3000/api/v1/users/dirtydishes/tokens >/tmp/g2-token.json
token=$(jq -er .sha1 /tmp/g2-token.json)
rm /tmp/g2-token.json
export GH_ENTERPRISE_TOKEN="$token"
E2E_TOKEN="$token" python3 /test/seed.py
test "$(gh api repos/dirtydishes/gfj-oracle/pulls/4 --jq .title)" = 'scenario: draft'
test "$(gh api repos/dirtydishes/gfj-oracle/pulls --jq '.[] | select(.number == 4) | .title')" = 'scenario: draft'
printf 'PASS REST draft titles on single and list routes\n'
E2E_TOKEN="$token" python3 /test/read-contract.py
cd /tmp/g2-seed
python3 /test/oracle-scripts/run.py forge.test /artifacts/oracle
python3 /test/oracle-scripts/compare.py /test/oracle /artifacts/oracle /artifacts/forgejo-pulls.json
# The repository is public, so a signed-in non-admin can read it without being a collaborator.
reader=$(curl -fsS -u reader:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"g2-reader","scopes":["read:repository","read:user","read:issue","read:organization"]}' \
    http://forgejo:3000/api/v1/users/reader/tokens | jq -er .sha1)
GH_ENTERPRISE_TOKEN="$reader" python3 /test/oracle-scripts/run.py forge.test /artifacts/oracle-reader
python3 /test/oracle-scripts/compare-viewers.py /artifacts/oracle /artifacts/oracle-reader
# With main protected, the reader gets the branch summary and the admin the full rule; compare them.
protection=http://forgejo:3000/api/v1/repos/dirtydishes/gfj-oracle/branch_protections
curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' -o /dev/null \
    -d '{"rule_name":"main","enable_status_check":true,"status_check_contexts":["ci / check (pull_request)","ci / lint (pull_request)"]}' "$protection"
python3 /test/oracle-scripts/run.py forge.test /artifacts/oracle-protected
GH_ENTERPRISE_TOKEN="$reader" python3 /test/oracle-scripts/run.py forge.test /artifacts/oracle-protected-reader
python3 /test/oracle-scripts/compare-viewers.py --protected /artifacts/oracle-protected /artifacts/oracle-protected-reader
curl -fsS -X DELETE -H "Authorization: token $token" "$protection/main"
python3 /test/write-seed.py
python3 /test/oracle-scripts/compare-writes.py /test/oracle/writes /artifacts/writes
# @me resolves to the reader too, not only to nobody: after the writes, so numbering is undisturbed.
curl -fsS -u reader:e2e-disposable-password -H 'Content-Type: application/json' -o /dev/null \
    -d '{"title":"reader issue"}' http://forgejo:3000/api/v1/repos/dirtydishes/gfj-oracle/issues
mine=$(GH_ENTERPRISE_TOKEN="$reader" gh issue list -R forge.test/dirtydishes/gfj-oracle --author @me --state all --json title --jq '[.[].title]')
test "$mine" = '["reader issue"]'
printf 'PASS @me is the reader for a non-admin\n'
