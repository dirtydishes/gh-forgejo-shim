#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$#" -ne 0 ]; then echo 'capture.sh accepts no commands or arguments' >&2; exit 2; fi
gh --version | head -n 1 | grep -q '^gh version 2.103.3 ' || { echo 'gh 2.103.3 required' >&2; exit 2; }
mkdir -p .local fixtures/oracle
unset GH_DEBUG GH_HOST GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
export PYTHONDONTWRITEBYTECODE=1
export GH_TOKEN
GH_TOKEN=$(gh auth token --hostname github.com)
capture=$(mktemp -d "$PWD/.local/oracle-capture.XXXXXX")
trap 'rm -rf "$capture"' EXIT
# This fixed GET supplies titles, bodies, branch names and states for the local twin.
gh api --hostname github.com --method GET 'repos/dirtydishes/gfj-oracle/pulls?state=all&per_page=100' | python3 scripts/oracle/seed-snapshot.py | python3 scripts/oracle/scrub.py >"$capture/seed.json"
# Fixed issue GETs keep the shared PR/issue numbering in sync.
for number in 11 12; do
    gh api --hostname github.com --method GET "repos/dirtydishes/gfj-oracle/issues/$number" --jq '{number: .number, title: .title, body: .body, state: .state}' | python3 scripts/oracle/scrub.py >"$capture/issue-$number.seed.json"
done
gh api --hostname github.com --method GET repos/dirtydishes/gfj-oracle/readme -H 'Accept: application/vnd.github.raw+json' | python3 scripts/oracle/scrub.py >"$capture/readme.md"
gh api --hostname github.com --method GET repos/dirtydishes/gfj-oracle --jq '{description: .description, private: .private}' | python3 scripts/oracle/scrub.py >"$capture/repository.seed.json"
scripts/oracle/capture-schema.sh >"$capture/schema-supplement.json"
python3 scripts/oracle/run.py github.com "$capture"
cp "$capture/"* fixtures/oracle/
printf 'Captured read-only oracle cases for dirtydishes/gfj-oracle\n'
