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
curl -fsS -X PUT -H "Authorization: token $token" -H 'Content-Type: application/json' -d '{"permission":"read"}' \
    http://forgejo:3000/api/v1/repos/dirtydishes/gfj-oracle/collaborators/reader
reader=$(curl -fsS -u reader:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"g2-reader","scopes":["read:repository","read:user","read:issue","read:organization"]}' \
    http://forgejo:3000/api/v1/users/reader/tokens | jq -er .sha1)
GH_ENTERPRISE_TOKEN="$reader" python3 /test/oracle-scripts/run.py forge.test /artifacts/oracle-reader
python3 /test/oracle-scripts/compare-viewers.py /artifacts/oracle /artifacts/oracle-reader
python3 /test/write-seed.py
python3 /test/oracle-scripts/compare-writes.py /test/oracle/writes /artifacts/writes
