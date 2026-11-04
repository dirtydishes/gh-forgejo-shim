#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
exec scripts/contract/seed-forgejo.sh python3 scripts/contract/ratchet.py
