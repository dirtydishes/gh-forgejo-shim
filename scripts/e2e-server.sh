#!/usr/bin/env bash
# Server mode end to end: the shipped image and nginx snippet in front of Forgejo 15, used by real
# gh 2.103.3 and Claude's REST lookups. Also validates the shipped Caddyfile.
set -euo pipefail
cd "$(dirname "$0")/.."
run="fga-server-e2e-$$"
cleanup() {
    result=$?
    trap - EXIT
    if [ "$result" -ne 0 ]; then
        for c in gateway nginx; do docker logs --tail 40 "$run-$c" 2>&1 || true; done
    fi
    for c in client nginx gateway forgejo cert; do docker rm -fv "$run-$c" >/dev/null 2>&1 || true; done
    docker network rm "$run" >/dev/null 2>&1 || true
    docker volume rm "$run-tls" >/dev/null 2>&1 || true
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
docker run --rm -v "$PWD/packaging/gateway/caddy/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 \
    caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
printf 'PASS the shipped Caddyfile validates\n'
docker build -q -f packaging/gateway/Dockerfile -t fga-server:e2e . >/dev/null
docker build -q -f scripts/e2e/Dockerfile -t fga-g2a-e2e:local . >/dev/null
docker network create "$run" >/dev/null
docker volume create "$run-tls" >/dev/null
docker run --rm --name "$run-cert" -v "$run-tls:/tls" fga-g2a-e2e:local openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout /tls/key.pem -out /tls/cert.pem -days 1 -subj /CN=forge.test \
    -addext subjectAltName=DNS:forge.test >/dev/null 2>&1
docker run -d --name "$run-forgejo" --network "$run" --network-alias forgejo \
    -e FORGEJO__security__INSTALL_LOCK=true -e FORGEJO__database__DB_TYPE=sqlite3 \
    -e FORGEJO__server__ROOT_URL=https://forge.test/ -e FORGEJO__server__DOMAIN=forge.test \
    -e FORGEJO__service__DISABLE_REGISTRATION=true -e FORGEJO__mailer__ENABLED=false \
    -e FORGEJO__repository__DEFAULT_BRANCH=main codeberg.org/forgejo/forgejo:15 >/dev/null
for attempt in $(seq 1 90); do
    docker exec "$run-forgejo" wget -q -O /dev/null http://localhost:3000/api/v1/version 2>/dev/null && break
    [ "$attempt" -eq 90 ] && { echo 'Forgejo did not become ready' >&2; exit 1; }
    sleep 1
done
docker exec --user git "$run-forgejo" forgejo --config /data/gitea/conf/app.ini admin user create \
    --username e2e --password e2e-disposable-password --email e2e@example.invalid --admin --must-change-password=false >/dev/null
docker run -d --name "$run-gateway" --network "$run" --network-alias gateway \
    -e FGA_FORGEJO_URL=http://forgejo:3000 -e FGA_PUBLIC_URL=https://forge.test fga-server:e2e >/dev/null
docker run -d --name "$run-nginx" --network "$run" --network-alias forge.test -v "$run-tls:/tls:ro" \
    -v "$PWD/packaging/gateway/nginx/fga-locations.conf:/etc/nginx/fga-locations.conf:ro" \
    -v "$PWD/scripts/e2e/server-nginx.conf:/etc/nginx/conf.d/default.conf:ro" nginx:1.29 >/dev/null
docker run --name "$run-client" --network "$run" -v "$run-tls:/tls:ro" \
    -v "$PWD/scripts/e2e/server-client.sh:/test/server-client.sh:ro" fga-g2a-e2e:local bash /test/server-client.sh
printf 'PASS server-mode e2e: the shipped image and nginx snippet serve gh and Claude for Forgejo 15\n'
