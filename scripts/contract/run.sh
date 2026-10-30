#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
exec scripts/contract/seed-forgejo.sh cargo test -p gh-forgejo-shim --locked --test contract -- --ignored --nocapture
