#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
run="fga-g2a-$$"
image="fga-g2a-e2e:local"
artifacts="$PWD/.local/e2e/$run"
mkdir -p "$artifacts"
cleanup() {
    result=$?
    trap - EXIT
    if [ "$result" -ne 0 ]; then
        docker logs "$run-gateway" 2>&1 || true
        docker logs --tail 60 "$run-forgejo" 2>&1 || true
    fi
    for container in client gateway forgejo cert; do docker rm -fv "$run-$container" >/dev/null 2>&1 || true; done
    docker network rm "$run" >/dev/null 2>&1 || true
    docker volume rm "$run-tls" >/dev/null 2>&1 || true
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
PYTHONDONTWRITEBYTECODE=1 python3 scripts/oracle/test_oracle.py
PYTHONDONTWRITEBYTECODE=1 python3 scripts/e2e/test_seed.py
docker info >/dev/null
docker build -f scripts/e2e/Dockerfile -t "$image" .
docker network create "$run" >/dev/null
docker volume create "$run-tls" >/dev/null
docker run --rm --name "$run-cert" -v "$run-tls:/tls" "$image" openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout /tls/key.pem -out /tls/cert.pem -days 1 -subj /CN=forge.test \
    -addext subjectAltName=DNS:forge.test >/dev/null 2>&1
docker run -d --name "$run-forgejo" --network "$run" --network-alias forgejo \
    -e FORGEJO__security__INSTALL_LOCK=true -e FORGEJO__database__DB_TYPE=sqlite3 \
    -e FORGEJO__server__ROOT_URL=https://forge.test/ -e FORGEJO__server__DOMAIN=forge.test \
    -e FORGEJO__service__DISABLE_REGISTRATION=true -e FORGEJO__service__REGISTER_EMAIL_CONFIRM=false \
    -e FORGEJO__mailer__ENABLED=false -e FORGEJO__repository__DEFAULT_BRANCH=main \
    codeberg.org/forgejo/forgejo:15 >/dev/null
for attempt in $(seq 1 90); do
    if docker exec "$run-forgejo" wget -q -O /dev/null http://localhost:3000/api/v1/version 2>/dev/null; then break; fi
    if [ "$attempt" -eq 90 ]; then echo 'Forgejo did not become ready' >&2; exit 1; fi
    sleep 1
done
docker exec --user git "$run-forgejo" forgejo --config /data/gitea/conf/app.ini admin user create \
    --username e2e --password e2e-disposable-password --email e2e@example.invalid --admin --must-change-password=false >/dev/null
docker exec --user git "$run-forgejo" forgejo --config /data/gitea/conf/app.ini admin user create \
    --username dirtydishes --password e2e-disposable-password --email dirtydishes@users.noreply.invalid --admin --must-change-password=false >/dev/null
docker run -d --name "$run-gateway" --network "$run" --network-alias forge.test \
    -v "$run-tls:/tls:ro" -e FGA_FORGEJO_URL=http://forgejo:3000 -e FGA_PUBLIC_URL=https://forge.test \
    -e FGA_LISTEN=0.0.0.0:443 -e FGA_TLS_CERT=/tls/cert.pem -e FGA_TLS_KEY=/tls/key.pem \
    "$image" /usr/local/bin/fga-server >/dev/null
docker run --name "$run-client" --network "$run" -v "$run-tls:/tls:ro" \
    -v "$artifacts:/artifacts" "$image" bash -c 'bash /test/client.sh && bash /test/g2.sh'
printf 'PASS e2e: gh 2.103.3 and Claude lookups against Forgejo 15\n'
printf 'Captured query and results: %s\n' "$artifacts"
