# Pull request links

Every pull request URL the wrapper and the gateway print uses GitHub's layout,
`https://<forge>/<owner>/<repo>/pull/7`, not Forgejo's `/pulls/7`. ChatGPT and Codex.app find a PR
only by that `/pull/<number>` shape. Forgejo itself has no `/pull/` page:

- **Server mode** redirects `/pull/7` to Forgejo's `/pulls/7`, so every link opens.
- **Client-only installs:** the wrapper's own `--web` commands (`gh pr view --web`, `gh pr diff --web`,
  `gh pr comment --web`, and `gh issue view --web` for a PR) give Forgejo's `/pulls/7` page. A `/pull/7` link opened anywhere else, such as
  ChatGPT's "Open pull request in browser", gets a 404 unless the forge redirects it.

If you run the forge, add the redirect to its reverse proxy. For nginx, in the forge's `server` block:

```nginx
location ~ "^/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/([0-9]+)(/(?:files|commits|checks))?/?$" {
    return 301 /$1/$2/pulls/$3$4$is_args$args;
}
```

For Nginx Proxy Manager, paste the same block into the forge's proxy host, under **Advanced**, below
anything already there.

For Caddy, in the forge's site block:

```caddyfile
@pull_link path_regexp pull ^/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/([0-9]+)(/(?:files|commits|checks))?/?$
redir @pull_link /{re.pull.1}/{re.pull.2}/pulls/{re.pull.3}{re.pull.4}{?query} 301
```

Both match only plain owner and repository names, so a crafted path can't redirect to another site.
They assume the forge is served at the root of its hostname; under a subpath, add the prefix to both
the pattern and the target.
Server mode's gateway does the same, and also redirects deeper paths under a PR.
