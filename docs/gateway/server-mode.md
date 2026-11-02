# Server mode

Server mode runs the gateway, `fga-server`, next to your Forgejo instance and mounts it on the forge's
own hostname. The forge then answers the GitHub Enterprise API at `https://<forge>/api/v3` and
`https://<forge>/api/graphql`. Anything that speaks to GitHub Enterprise can use it directly, with no
wrapper on the client machine.

**Use server mode if you use Claude.** Claude Code and Claude desktop look up a branch's pull request
by calling `https://<forge>/api/v3` themselves, outside of any `gh` process. Only a gateway on
the forge's hostname can answer them. The client-only wrapper still serves Claude's agent when it runs
`gh` in a shell.

You need control of the forge's reverse proxy. Hosted forges where you can't add routes, such as
Codeberg, can't use server mode.

## 1. Run the gateway

Build the image from this repository's root, then run it where it can reach Forgejo. The examples
below assume Forgejo listens on the host at `127.0.0.1:3000` and the proxy runs on the host:

```sh
docker build -f packaging/gateway/Dockerfile -t fga-server .
docker run -d --name fga-server --restart unless-stopped --network host \
    -e FGA_LISTEN=127.0.0.1:8080 \
    -e FGA_FORGEJO_URL=http://127.0.0.1:3000 \
    -e FGA_PUBLIC_URL=https://forge.example.com \
    fga-server
```

If Forgejo runs in a container instead, put the gateway on Forgejo's Docker network and publish it
on the host's loopback for a proxy on the host. Leave `FGA_LISTEN` unset: the image listens on
`0.0.0.0:8080` inside its container, which a published port or another container needs.

```sh
docker run -d --name fga-server --restart unless-stopped \
    --network <forgejo's network> -p 127.0.0.1:8080:8080 \
    -e FGA_FORGEJO_URL=http://<forgejo container>:3000 \
    -e FGA_PUBLIC_URL=https://forge.example.com \
    fga-server
```

- `FGA_FORGEJO_URL` is how the gateway reaches Forgejo, usually its internal address.
- `FGA_PUBLIC_URL` is the forge's own address, as users type it. The gateway builds links from it and
  answers only requests addressed to its host and port.
- `FGA_WIP_PREFIXES` must match Forgejo's `[repository.pull-request] WORK_IN_PROGRESS_PREFIXES` if you
  changed them (default `WIP:,[WIP]`).

The [gateway README](README.md) lists every setting.

## 2. Mount it on the forge's hostname

Route three kinds of path to the gateway, and everything else to Forgejo as before:

- `/api/v3/` and `/api/graphql`: the GitHub API;
- `/{owner}/{repo}/pull/{n}`: GitHub-style PR links, which the gateway redirects to Forgejo's
  `/pulls/{n}`.

Put these routes only in the forge's own site or `server` block. In server mode, that block decides
which host a request is for: the client connected to the forge's name, so its token is already with
the forge. Pass the client's `Host` header through unchanged, and the gateway checks it again. If the
routes end up in a block that serves other names too, or in the default server, the gateway refuses
any request for another host with `421`, so another host's token never reaches Forgejo.

A client can name one host in `Host` and another in its request line or HTTP/2 headers, and a proxy
forwards only one of them. The gateway can't check what the proxy drops. That only sends the client's
own token to the forge it connected to.

**nginx.** Define the upstream once, then include the shipped
[locations snippet](../../packaging/gateway/nginx/fga-locations.conf) in the forge's `server` block:

```nginx
upstream fga { server 127.0.0.1:8080; }

server {
    server_name forge.example.com;
    # ... your TLS settings ...
    include /etc/nginx/fga-locations.conf;
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $http_host;
    }
}
```

The snippet uses `$http_host`, because `$host` drops a non-default port. It answers `400` to a request
whose first line names a host (`GET https://forge.example.com:8443/api/v3/user`), because nginx would
forward it without that host. Clients send such lines only to forward proxies.

**Caddy.** Copy the [site block](../../packaging/gateway/caddy/Caddyfile) and set its hostname and two
upstreams, or export `FORGE_SITE`, `FGA_UPSTREAM` and `FORGEJO_UPSTREAM`. Caddy passes `Host`
through by default, and its site block answers only the forge's hostname.

**Nginx Proxy Manager.** NPM's nginx runs inside the NPM container, so the gateway must be on a Docker
network NPM shares, reached by its container name, with no published port. Forgejo must be reachable
from that network too:

```sh
docker run -d --name fga-server --restart unless-stopped \
    --network <NPM's network> \
    -e FGA_FORGEJO_URL=http://<forgejo container>:3000 \
    -e FGA_PUBLIC_URL=https://forge.example.com \
    fga-server
```

Start it before the next step: nginx looks up that name when NPM saves the host, and if it can't, NPM
takes the whole proxy host offline, forge included.

Then open the forge's proxy host, choose **Advanced**, and add the contents of the nginx locations
snippet, with `http://fga` replaced by `http://fga-server:8080`. Add it **below** anything already in
that field; don't replace it. Leave the host's main forward settings pointing at Forgejo.

With **Cache Assets** on, NPM's asset location comes before the Advanced field and wins for paths that
end like a file it caches. A PR link such as `/{owner}/{repo}/pull/1/files/logo.svg` then reaches
Forgejo, which answers `404`. The API routes aren't affected, and GitHub's own PR links don't end that
way.

NPM's **Access Lists** don't cover locations added in the Advanced field. If the forge relies on an
Access List to limit who can reach it, the gateway's routes are open to everyone. Restrict them with
server-level rules in the Advanced field instead (`if`/`return` before the snippet), which run before
nginx picks a location, or keep the forge behind a network boundary.

## 3. Check it

Every API route needs a Forgejo token:

```sh
curl -fsS -H "Authorization: token <your Forgejo token>" https://forge.example.com/api/v3/meta
```

The reply is `{"installed_version":"3.19.0"}`. Without a token the gateway answers `401`, which also
shows the route reaches it. A `/pull/1` link on any repository should open Forgejo's PR page.

## 4. Use it

- **gh:** `gh auth login --hostname forge.example.com` with a Forgejo token. Plain `gh` then works in
  the forge's repositories, with no wrapper. Grant the token `read:user`, `write:repository`,
  `write:issue` and `read:organization`.
- **Claude Code and Claude desktop:** log `gh` in as above. Both get their token from `gh`, then look up
  the branch's pull request on the forge directly.
- **ChatGPT desktop:** it drives `gh`, so the same login serves it.

## Limits

- One gateway serves one forge.
- The gateway serves the API surface gh, Claude and ChatGPT use. Anything outside it gets a
  GitHub-shaped `404`.

`scripts/e2e-server.sh` runs this setup end to end: the shipped image and nginx snippet in front of
Forgejo 15, used by real gh and Claude's REST lookups.
