#!/usr/bin/env bash
# Rehearse twin-seed.py against a disposable Forgejo 15 holding a copy of the live twin's main:
# seed it, then run it again to prove a resumed run writes nothing. No runner: CI stays unreported.
set -euo pipefail
cd "$(dirname "$0")/../.."
run="gfj-twin-rehearse-$$"
port=$((20000 + $$ % 10000))
work=$(mktemp -d)
cleanup() { [ -n "${KEEP:-}" ] || docker rm -fv "$run" >/dev/null 2>&1 || true; rm -rf "$work"; }
trap cleanup EXIT
docker run -d --name "$run" -p "127.0.0.1:$port:3000" \
    -e FORGEJO__security__INSTALL_LOCK=true -e FORGEJO__database__DB_TYPE=sqlite3 \
    -e FORGEJO__server__ROOT_URL="http://127.0.0.1:$port/" -e FORGEJO__repository__DEFAULT_BRANCH=main \
    -e FORGEJO__service__DISABLE_REGISTRATION=true -e FORGEJO__mailer__ENABLED=false \
    codeberg.org/forgejo/forgejo:15 >/dev/null
for attempt in $(seq 1 90); do
    curl -fsS "http://127.0.0.1:$port/api/v1/version" >/dev/null 2>&1 && break
    [ "$attempt" -eq 90 ] && { echo 'Forgejo did not become ready' >&2; exit 1; }
    sleep 1
done
docker exec --user git "$run" forgejo --config /data/gitea/conf/app.ini admin user create --admin \
    --username dirtydishes --password rehearse-disposable-password --email dirtydishes@example.invalid \
    --must-change-password=false >/dev/null
token=$(curl -fsS -u dirtydishes:rehearse-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"rehearse","scopes":["all"]}' "http://127.0.0.1:$port/api/v1/users/dirtydishes/tokens" | jq -er .sha1)
curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' \
    -d '{"name":"gfj-oracle","private":false}' "http://127.0.0.1:$port/api/v1/user/repos" >/dev/null
# A read-only copy of the live twin's main, so branches start from the same tree.
git clone -q --branch main --single-branch https://git.dirtydishes.dev/dirtydishes/gfj-oracle.git "$work/main"
git -C "$work/main" -c "http.extraHeader=Authorization: token $token" \
    push -q "http://127.0.0.1:$port/dirtydishes/gfj-oracle.git" main
# Forgejo processes the push asynchronously; the repository lists PRs only once it isn't empty.
for attempt in $(seq 1 30); do
    curl -fsS "http://127.0.0.1:$port/api/v1/repos/dirtydishes/gfj-oracle" | jq -e '.empty == false' >/dev/null && break
    [ "$attempt" -eq 30 ] && { echo 'the copied main did not settle' >&2; exit 1; }
    sleep 1
done
TWIN_TOKEN="$token" python3 scripts/oracle/twin-seed.py --rehearse "http://127.0.0.1:$port" | tee "$work/first.log"
TWIN_TOKEN="$token" python3 scripts/oracle/twin-seed.py --rehearse "http://127.0.0.1:$port" | tee "$work/second.log"
if grep -E '^(created|merged|closed|reviewed) ' "$work/second.log"; then
    echo 'a resumed run wrote again' >&2; exit 1
fi
api() { curl -fsS -H "Authorization: token $token" "http://127.0.0.1:$port/api/v1/repos/dirtydishes/gfj-oracle/$1"; }
api 'pulls?state=all&limit=50' | jq -c '[.[] | {n: .number, s: .state, m: .merged, d: .draft}] | sort_by(.n)'
api 'pulls/13/commits' | jq -c '[.[].commit.message]'
api 'pulls/14/reviews' | jq -c '[.[] | {u: .user.login, s: .state, c: .comments_count}]'
api 'pulls/15/reviews' | jq -c '[.[] | {u: .user.login, s: .state}]'
echo 'PASS twin-seed rehearsal: seeded once, resumed without writing'
