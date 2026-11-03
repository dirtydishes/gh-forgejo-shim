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
        for c in gateway nginx caddy; do docker logs --tail 40 "$run-$c" 2>&1 || true; done
    fi
    for c in client launcher caddy nginx gateway forgejo cert; do docker rm -fv "$run-$c" >/dev/null 2>&1 || true; done
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
# Every gateway route sends the client's Host, which Caddy would rewrite for an HTTPS upstream.
hosts=$(docker run --rm -e FGA_UPSTREAM=https://gateway.test:8443 \
    -v "$PWD/packaging/gateway/caddy/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 \
    caddy adapt --config /etc/caddy/Caddyfile --adapter caddyfile 2>/dev/null |
    grep -o '"Host":\["{http.request.hostport}"\]' | wc -l | tr -d ' ')
[ "$hosts" = 3 ] || { echo "FAIL the Caddyfile sets Host on $hosts of 3 gateway routes" >&2; exit 1; }
printf 'PASS the Caddyfile sends the client'"'"'s Host on all 3 gateway routes\n'
docker build -q -f packaging/gateway/Dockerfile -t fga-server:e2e . >/dev/null
docker build -q -f scripts/e2e/Dockerfile -t fga-g2a-e2e:local . >/dev/null
docker network create "$run" >/dev/null
docker volume create "$run-tls" >/dev/null
# A CA and a forge.test leaf it signs: rustls, which the shim's gateway client uses, refuses a
# self-signed CA certificate as a server's own.
docker run --rm --name "$run-cert" -v "$run-tls:/tls" fga-g2a-e2e:local sh -c '
    openssl req -x509 -newkey rsa:2048 -nodes -keyout /tls/ca-key.pem -out /tls/ca.pem -days 1 -subj "/CN=e2e CA" &&
    openssl req -newkey rsa:2048 -nodes -keyout /tls/key.pem -out /tls/leaf.csr -subj /CN=forge.test &&
    printf "subjectAltName=DNS:forge.test\nbasicConstraints=CA:FALSE\n" > /tls/leaf.ext &&
    openssl x509 -req -in /tls/leaf.csr -CA /tls/ca.pem -CAkey /tls/ca-key.pem -CAcreateserial \
        -days 1 -extfile /tls/leaf.ext -out /tls/cert.pem' >/dev/null 2>&1
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
# The shipped Caddyfile too, over plain HTTP: the same routes, through Caddy's own matchers.
docker run -d --name "$run-caddy" --network "$run" --network-alias caddy \
    -e FORGE_SITE=http://forge.test -e FGA_UPSTREAM=gateway:8080 -e FORGEJO_UPSTREAM=forgejo:3000 \
    -v "$PWD/packaging/gateway/caddy/Caddyfile:/etc/caddy/Caddyfile:ro" caddy:2 >/dev/null
docker run --name "$run-client" --network "$run" -v "$run-tls:/tls:ro" \
    -v "$PWD/scripts/e2e/server-client.sh:/test/server-client.sh:ro" fga-g2a-e2e:local bash /test/server-client.sh
docker run --name "$run-launcher" --network "$run" -v "$run-tls:/tls:ro" \
    -v "$PWD/scripts/e2e/launcher-client.sh:/test/launcher-client.sh:ro" fga-g2a-e2e:local bash /test/launcher-client.sh
printf 'PASS server-mode e2e: the shipped image and nginx snippet serve gh and Claude for Forgejo 15\n'
printf 'PASS client-mode e2e: the shim launches real gh against the same forge\n'
