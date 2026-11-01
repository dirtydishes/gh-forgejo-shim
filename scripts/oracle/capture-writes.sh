#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
if [ "$#" -ne 0 ]; then echo 'capture-writes.sh accepts no commands or arguments' >&2; exit 2; fi
gh --version | head -n 1 | grep -q '^gh version 2.103.3 ' || { echo 'gh 2.103.3 required' >&2; exit 2; }
unset GH_DEBUG GH_HOST GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN
export PYTHONDONTWRITEBYTECODE=1 GH_TOKEN
GH_TOKEN=$(gh auth token --hostname github.com)
mkdir -p .local fixtures/oracle/writes
capture=$(mktemp -d "$PWD/.local/write-capture.XXXXXX")
# Keep an interrupted ledger for recovery. Replace fixtures only on complete success.
python3 scripts/oracle/writes.py github.com "$capture"
cp "$capture/"* fixtures/oracle/writes/
printf 'Captured complete write oracle in %s\n' "$capture"
