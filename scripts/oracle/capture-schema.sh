#!/usr/bin/env bash
# This fixed introspection query reads schema metadata only.
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$#" -ne 0 ]; then echo 'capture-schema.sh accepts no arguments' >&2; exit 2; fi
unset GH_DEBUG GH_HOST GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
export GH_TOKEN
GH_TOKEN=$(gh auth token --hostname github.com)
mkdir -p .local
capture=$(mktemp -d "$PWD/.local/schema.XXXXXX")
trap 'rm -rf "$capture"' EXIT
for name in Issue IssueType IssueTypeColor SearchType; do
    gh api --hostname github.com graphql -F query=@scripts/oracle/schema-supplement.graphql -f name="$name" >"$capture/$name.json"
done
python3 - "$capture" <<'PYTHON' | python3 scripts/oracle/scrub.py
import json, sys
from pathlib import Path
selected = {'Issue': {'issueType', 'blockedBy', 'blocking'}, 'IssueType': {'id', 'name', 'description', 'color'}}
data = {}
for p in sorted(Path(sys.argv[1]).glob('*.json')):
    value = json.loads(p.read_text())['data']['__type']
    if p.stem in selected:
        value['fields'] = [f for f in value['fields'] if f['name'] in selected[p.stem]]
    data[p.stem] = value
print(json.dumps({'data': data}, indent=2))
PYTHON
