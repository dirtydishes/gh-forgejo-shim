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
# A read-only copy of the live twin's main as it was before the seed, whose merges added notes the
# plan writes again, so branches start from the same tree.
base=a3972cc9bd36f1380cf266f1e9dfed892ad0485e
git clone -q --branch main --single-branch https://git.dirtydishes.dev/dirtydishes/gfj-oracle.git "$work/main"
git -C "$work/main" -c "http.extraHeader=Authorization: token $token" \
    push -q "http://127.0.0.1:$port/dirtydishes/gfj-oracle.git" "$base:refs/heads/main"
# Forgejo processes the push asynchronously; the repository lists PRs only once it isn't empty.
for attempt in $(seq 1 30); do
    curl -fsS "http://127.0.0.1:$port/api/v1/repos/dirtydishes/gfj-oracle" | jq -e '.empty == false' >/dev/null && break
    [ "$attempt" -eq 30 ] && { echo 'the copied main did not settle' >&2; exit 1; }
    sleep 1
done
api() { curl -fsS -H "Authorization: token $token" "http://127.0.0.1:$port/api/v1/repos/dirtydishes/gfj-oracle/$1"; }
# Stand in for the runner: fail #13's first commit once it exists, as the twin's CI would.
(
    for attempt in $(seq 1 300); do
        sha=$(api 'pulls/13' 2>/dev/null | jq -r '.head.sha // empty') || true
        if [ -n "$sha" ]; then
            curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' \
                -d '{"context":"ci / check (pull_request)","state":"failure"}' \
                "http://127.0.0.1:$port/api/v1/repos/dirtydishes/gfj-oracle/statuses/$sha" >/dev/null
            exit 0
        fi
        sleep 1
    done
) &
TWIN_TOKEN="$token" python3 scripts/oracle/twin-seed.py --rehearse "http://127.0.0.1:$port" | tee "$work/first.log"
TWIN_TOKEN="$token" python3 scripts/oracle/twin-seed.py --rehearse "http://127.0.0.1:$port" | tee "$work/second.log"
if grep -E '^(created|merged|closed|reviewed|pushed) ' "$work/second.log"; then
    echo 'a resumed run wrote again' >&2; exit 1
fi
expect() { # what, jq filter that must be true
    api "$1" | jq -e "$2" >/dev/null || { echo "FAIL $1: $2" >&2; api "$1" | jq -c . >&2; exit 1; }
    echo "PASS $1"
}
expect 'pulls?state=all&limit=50' '[.[] | [.number, .state, .merged, .draft]] | sort ==
    [[1,"open",false,false],[2,"open",false,false],[3,"open",false,false],[4,"open",false,true],[5,"open",false,false],
     [6,"closed",true,false],[7,"closed",false,false],[8,"closed",true,false],[9,"open",false,false],[10,"closed",true,false],
     [13,"open",false,false],[14,"open",false,false],[15,"open",false,false]]'
expect 'issues?state=all&type=issues' '[.[] | [.number, .state]] | sort == [[11,"open"],[12,"closed"]]'
expect 'pulls/13/commits' 'length == 2'
expect 'pulls/13/files' '[.[].filename] == []'
# The PR opened on a failing commit, and the fix landed on top of it (F4): the oldest commit
# failed. The head has no failure; with no runner here it has no statuses at all.
oldest=$(api 'pulls/13/commits' | jq -r '.[-1].sha')
head=$(api 'pulls/13/commits' | jq -r '.[0].sha')
expect "commits/$oldest/statuses" 'any(.[]; .context == "ci / check (pull_request)" and .status == "failure")'
expect "commits/$head/statuses" 'all(.[]; .status != "failure")'
expect 'pulls/3/commits' 'length == 2'
expect 'pulls/3/files' '[.[].filename] | sort == ["ci/slow","notes/pending.txt"]'
expect 'pulls/14/reviews' '[.[] | [.user.login, .state, .comments_count]] == [["gfj-e2e","APPROVED",1]]'
expect 'pulls/15/reviews' '[.[] | [.user.login, .state]] == [["gfj-e2e","REQUEST_CHANGES"]]'
curl -fsS -H "Authorization: token $token" "http://127.0.0.1:$port/api/v1/users/gfj-e2e" | jq -e '.is_admin == false' >/dev/null
echo 'PASS twin-seed rehearsal: every planned state, and a resumed run wrote nothing'
