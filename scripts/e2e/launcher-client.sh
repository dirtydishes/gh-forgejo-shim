#!/usr/bin/env bash
# Client mode as a user sees it (launcher spec §11, test 14): the shim installed as `gh` launches
# real gh for `gh pr checkout`, and git fetches the pull request from https://forge.test, a private
# repository behind nginx with TLS, using only the launcher's credential helper.
set -euo pipefail
cp /tls/ca.pem /usr/local/share/ca-certificates/forge-test.crt
update-ca-certificates >/dev/null 2>&1
check() { if eval "$1"; then printf 'PASS %s\n' "$2"; else printf 'FAIL %s\n' "$2" >&2; exit 1; fi; }
forgejo=http://forgejo:3000/api/v1
token=$(curl -fsS -u e2e:e2e-disposable-password -H 'Content-Type: application/json' \
    -d '{"name":"launcher-e2e","scopes":["all"]}' "$forgejo/users/e2e/tokens" | jq -er .sha1)
api() { curl -fsS -H "Authorization: token $token" -H 'Content-Type: application/json' "$@"; }

# A private repository, so git's fetch needs a credential, with one pull request from s/pass.
api -d '{"name":"launcher","default_branch":"main","private":true}' "$forgejo/user/repos" >/dev/null
mkdir /tmp/launcher-seed && cd /tmp/launcher-seed
git init -q -b main && git config user.name e2e && git config user.email e2e@example.invalid
printf 'main\n' > README.md && git add . && git commit -qm main
git checkout -qb s/pass && printf 'pass\n' > pass.txt && git add . && git commit -qm s/pass
head=$(git rev-parse s/pass)
git -c "http.extraHeader=Authorization: token $token" push -q http://forgejo:3000/e2e/launcher.git main s/pass
for attempt in $(seq 1 30); do
    api "$forgejo/repos/e2e/launcher" | jq -e '.empty == false' >/dev/null && break
    [ "$attempt" -eq 30 ] && { echo 'Forgejo still reports an empty repository' >&2; exit 1; }
    sleep 1
done
api -d '{"base":"main","head":"s/pass","title":"scenario: checkout"}' "$forgejo/repos/e2e/launcher/pulls" >/dev/null

# The user's side: the shim configured for forge.test, whose private CA it trusts through
# ca_file (the gateway's client keeps its own root store, so the system store isn't enough), no
# gh login (so gh launches rather than stepping aside), the shim first on PATH as gh, and no
# credential helper or prompt anywhere. git trusts the CA through the system store.
export HOME=/tmp/launcher-home
mkdir -p "$HOME/.config/gh-forgejo-shim" /tmp/launcher-bin
printf 'hosts = ["forge.test"]\n[host."forge.test"]\ntoken_env = "LAUNCHER_E2E_TOKEN"\nca_file = "/tls/ca.pem"\n' \
    > "$HOME/.config/gh-forgejo-shim/config.toml"
# The wrapper exactly as the shim's installer writes it (shim.rs, shim_script).
printf '#!/bin/sh\n# managed by gh-forgejo-shim\n# rollback-compatible marker: gh-forgejo-shim gh\n# rust gh-forgejo-shim wrapper\nexec /usr/local/bin/gh-forgejo-shim gh "$@"\n' \
    > /tmp/launcher-bin/gh
chmod 755 /tmp/launcher-bin/gh
export PATH="/tmp/launcher-bin:$PATH" FJ_SHIM_REAL_GH=/usr/local/bin/gh LAUNCHER_E2E_TOKEN="$token"
export GH_PROMPT_DISABLED=1 GIT_TERMINAL_PROMPT=0 TMPDIR=/tmp/launcher-tmp
# Without XDG_RUNTIME_DIR the run directory goes in TMPDIR, where the checks below look.
unset GH_TOKEN GITHUB_TOKEN GH_ENTERPRISE_TOKEN GITHUB_ENTERPRISE_TOKEN GH_HOST GH_CONFIG_DIR \
    GIT_ASKPASS SSH_ASKPASS GIT_CONFIG_PARAMETERS XDG_RUNTIME_DIR
mkdir -m 700 "$TMPDIR"
git config --global user.name e2e && git config --global user.email e2e@example.invalid
check '[ -z "$(git config --get-all credential.helper || true)" ] && [ -z "$(git config --get core.askPass || true)" ]' \
    'no credential helper or askpass is configured'
# The clone's credential lives only in this command's -c; nothing is stored.
git -c "http.extraHeader=Authorization: token $token" clone -q https://forge.test/e2e/launcher.git /tmp/launcher-checkout
cd /tmp/launcher-checkout
check '! git ls-remote origin >/dev/null 2>&1' 'without the launcher, git has no credential for the private repository'

# gh pr checkout: real gh through the launcher; its git fetch asks the launcher's helper.
GIT_TRACE=/tmp/launcher-trace gh pr checkout 1
check '[ "$(git rev-parse --abbrev-ref HEAD)" = s/pass ] && [ "$(git rev-parse HEAD)" = "$head" ]' \
    'gh pr checkout 1 checks out the pull request head'
# git quotes the helper's path: run_command: '"<run dir>/gh" auth git-credential get'
check 'grep -qE "run_command: .*/gfj-[0-9a-f]{16}/gh.? auth git-credential get" /tmp/launcher-trace' 'git asked the launcher'"'"'s helper'
check '[ -z "$(ls -A "$TMPDIR")" ]' 'the run directory is gone'
# Every helper git ran: a run_command naming a credential helper or askpass must be the launcher's.
only_launcher() {
    ! grep -E "run_command: .*(credential|askpass)" "$1" |
        grep -vqE "run_command: .*/gfj-[0-9a-f]{16}/gh.? auth git-credential (get|store|erase)"
}
leftover() { grep -rlF "$token" "$HOME" /tmp/launcher-checkout "$TMPDIR" /tmp/launcher-trace* 2>/dev/null; }
check '[ -z "$(leftover)" ]' 'the token is nowhere in the checkout, the home directory or the trace'

# Competing helpers, as a user's own setup has them: a generic one and real gh's own, which
# would answer from gh's hosts.yml. Neither may run inside a launch.
printf '#!/bin/sh\necho "$*" >> /tmp/launcher-other-helper\n' > /tmp/launcher-bin/other-helper
chmod 755 /tmp/launcher-bin/other-helper
git config --global credential.helper /tmp/launcher-bin/other-helper
git config --global --add credential.https://forge.test.helper '!/usr/local/bin/gh auth git-credential'
git checkout -q main
GIT_TRACE=/tmp/launcher-trace-helpers gh pr checkout 1 --detach
check 'only_launcher /tmp/launcher-trace-helpers && [ ! -e /tmp/launcher-other-helper ]' \
    'with other helpers configured, only the launcher'"'"'s helper ran'
check 'only_launcher /tmp/launcher-trace' 'the first checkout ran only the launcher'"'"'s helper too'
git config --global --unset-all credential.helper
git config --global --unset-all credential.https://forge.test.helper

# The other launched flags, each git-side.
git checkout -q main
gh pr checkout 1 --branch local-pass
check '[ "$(git rev-parse --abbrev-ref HEAD)" = local-pass ] && [ "$(git rev-parse HEAD)" = "$head" ]' \
    'gh pr checkout --branch names the local branch'
git checkout -q main
gh pr checkout 1 --detach
check '[ "$(git rev-parse --abbrev-ref HEAD)" = HEAD ] && [ "$(git rev-parse HEAD)" = "$head" ]' \
    'gh pr checkout --detach leaves HEAD detached at the pull request head'
git checkout -q s/pass && git commit -q --allow-empty -m 'local only' && git checkout -q main
gh pr checkout 1 --force
check '[ "$(git rev-parse --abbrev-ref HEAD)" = s/pass ] && [ "$(git rev-parse HEAD)" = "$head" ]' \
    'gh pr checkout --force resets a diverged local branch to the pull request head'
gh pr checkout 1 --detach -R forge.test/e2e/launcher
check '[ "$(git rev-parse HEAD)" = "$head" ]' 'gh pr checkout -R names the repository'
check '[ -z "$(ls -A "$TMPDIR")" ] && [ -z "$(leftover)" ]' 'no run directory or token is left after six launches'
printf 'PASS client-mode e2e: a launched gh pr checkout fetches over TLS with only the launcher'"'"'s helper\n'
