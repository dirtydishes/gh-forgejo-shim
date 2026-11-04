//! The gateway router. It never prints to stdout or stderr: launched in-process, gh owns both.
//! A panic still reaches stderr through the panic hook, which the embedding process owns.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]
pub mod config;
mod rest;
mod runs;

use async_graphql::Request;
use axum::{
    Extension, Json, Router,
    extract::{Request as HttpRequest, State},
    http::{
        StatusCode,
        header::{AUTHORIZATION, HOST},
        uri::Authority,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use fga_core::{
    forgejo::{Error, Forgejo, Session},
    graphql::{self, GithubSchema},
    map::WipPrefixes,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use url::Url;

/// One forge the gateway serves: requests addressed to `name` go to `forgejo`, and links use
/// `public_url`.
pub struct Host {
    pub public_url: Url,
    pub forgejo: Forgejo,
    pub wip_prefixes: WipPrefixes,
    /// Another scheme and authority requests may name, besides `public_url`'s.
    name: Url,
    /// When set, the only credential accepted for this host. A launcher hands each host's token
    /// over; server mode leaves it unset and lets Forgejo judge every user's token.
    expected_token: Option<[u8; 32]>,
    /// When set, the token sent to Forgejo in place of the admitted one.
    upstream_token: Option<String>,
}

impl Host {
    pub fn new(public_url: Url, forgejo: Forgejo, wip_prefixes: WipPrefixes) -> Self {
        Self {
            name: public_url.clone(),
            public_url,
            forgejo,
            wip_prefixes,
            expected_token: None,
            upstream_token: None,
        }
    }

    /// Also answer to `authority`, keeping `public_url` for links. gh drops a remote's port, so a
    /// forge on `:3443` is asked for as plain `forge.test`, and as `forge.test:3443` when the user
    /// writes the port.
    pub fn answering_to(self, authority: &str) -> Result<Self, url::ParseError> {
        let name = Url::parse(&format!("{}://{authority}", self.public_url.scheme()))?;
        if name.host().is_none() || name.path() != "/" || !name.username().is_empty() {
            return Err(url::ParseError::InvalidDomainCharacter);
        }
        Ok(Self { name, ..self })
    }

    pub fn expecting(self, token: String) -> Self {
        Self {
            expected_token: Some(Sha256::digest(token).into()),
            ..self
        }
    }

    /// Send `token` to Forgejo in place of the admitted one. For a forge on a custom port, gh's API
    /// calls present a random per-call token, while git keeps the real one under the host:port key
    /// (launcher spec §3); only that per-call token is admitted, and the real one goes upstream.
    /// `AppState::new` refuses it without an expected token, which would admit any caller.
    pub fn upstream_token(self, token: String) -> Self {
        Self {
            upstream_token: Some(token),
            ..self
        }
    }

    /// Whether `authorization` may be used for this host at all. Digests compare in constant time
    /// and reveal nothing about the expected token's length.
    fn admits(&self, authorization: &str) -> bool {
        let Some(expected) = &self.expected_token else {
            return true;
        };
        let presented = authorization
            .split_once(' ')
            .filter(|(scheme, _)| {
                scheme.eq_ignore_ascii_case("token") || scheme.eq_ignore_ascii_case("bearer")
            })
            .map_or("", |(_, token)| token);
        // A missing header or another scheme presents no token, which matches nothing, not even
        // an empty expected token.
        if presented.is_empty() {
            return false;
        }
        let presented: [u8; 32] = Sha256::digest(presented).into();
        presented
            .iter()
            .zip(expected)
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
    }
}

#[derive(Clone)]
pub struct AppState {
    hosts: Arc<[Arc<Host>]>,
    pub ghes_version: String,
    schema: GithubSchema,
}

impl AppState {
    /// Refuses an empty set, and two hosts answering to the same host and port: the second could
    /// never be reached.
    pub fn new(hosts: Vec<Host>, ghes_version: String) -> Result<Self, &'static str> {
        let keys = |host: &Host| {
            [&host.name, &host.public_url].map(|url| {
                (
                    url.host().map(|h| h.to_owned()),
                    url.port_or_known_default(),
                )
            })
        };
        let overlap = |a: &Host, b: &Host| keys(a).iter().any(|key| keys(b).contains(key));
        if hosts.is_empty() {
            return Err("the gateway needs at least one host");
        }
        if hosts
            .iter()
            .any(|host| host.upstream_token.is_some() && host.expected_token.is_none())
        {
            return Err("a host that sends its own token upstream must expect one from clients");
        }
        if hosts
            .iter()
            .enumerate()
            .any(|(i, a)| hosts[..i].iter().any(|b| overlap(a, b)))
        {
            return Err("two hosts answer to the same host and port");
        }
        Ok(Self {
            hosts: hosts.into_iter().map(Arc::new).collect(),
            ghes_version,
            schema: graphql::schema(),
        })
    }
}

pub fn app(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/v3/", get(rest::root))
        .route("/api/v3/meta", get(rest::meta))
        .route("/api/v3/rate_limit", get(rest::rate_limit))
        .route("/api/v3/user", get(rest::user))
        .route("/api/v3/repos/{owner}/{repo}", get(rest::repository))
        .route("/api/v3/repos/{owner}/{repo}/pulls", get(rest::pulls))
        .route(
            "/api/v3/repos/{owner}/{repo}/pulls/{number}",
            get(rest::pull),
        )
        .route("/api/v3/repos/{owner}/{repo}/readme", get(rest::readme))
        .route(
            "/api/v3/repos/{owner}/{repo}/git/refs/{*reference}",
            delete(rest::delete_branch).fallback(|| async { rest_error(Error::NotFound) }),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/commits/{sha}/pulls",
            get(rest::commit_pulls),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/commits/{sha}/status",
            get(rest::commit_status),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/commits/{sha}/check-runs",
            get(rest::check_runs),
        )
        .route("/api/v3/repos/{owner}/{repo}/actions/runs", get(runs::runs))
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/runs/{id}",
            get(runs::run),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/runs/{id}/jobs",
            get(runs::jobs),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/runs/{id}/artifacts",
            get(runs::artifacts),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/workflows",
            get(runs::workflows),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/workflows/{id}",
            get(runs::workflow),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/workflows/{id}/runs",
            get(runs::workflow_runs),
        )
        .route("/api/graphql", post(graphql_request))
        .fallback(|| async { rest_error(Error::NotFound) })
        .layer(middleware::from_fn(authenticate));
    Router::new()
        // Browsers follow these links without a token.
        .route("/{owner}/{repo}/pull/{number}", get(pull_link))
        .route("/{owner}/{repo}/pull/{number}/", get(pull_link))
        .route("/{owner}/{repo}/pull/{number}/{*rest}", get(pull_link))
        .merge(api)
        .layer(middleware::from_fn_with_state(state.clone(), select_host))
        .with_state(state)
}

/// gh and ChatGPT link PRs as GitHub does, `/{o}/{r}/pull/{n}`, which Forgejo doesn't serve, so
/// send the browser to Forgejo's `/pulls/{n}` (MAPPING §8). This answers without a token, so the
/// owner and repository must look like Forgejo names: anything else, such as an empty, encoded or
/// backslashed segment that a browser would read as another origin, gets 404. `api` is Forgejo's
/// own. A fragment stays with the browser across the redirect.
async fn pull_link(uri: axum::http::Uri) -> Response {
    let name = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    let segments: Vec<&str> = uri.path().split('/').collect();
    let [_, owner, repo, "pull", number, rest @ ..] = segments.as_slice() else {
        return rest_error(Error::NotFound);
    };
    if !name(owner)
        || !name(repo)
        || owner.eq_ignore_ascii_case("api")
        || number.is_empty()
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return rest_error(Error::NotFound);
    }
    // The rest names a page of the PR, such as `files`: plain segments only, so `Location` can't
    // reach another path on the forge. A trailing slash leaves one empty last segment.
    let (last, middle) = rest
        .split_last()
        .map_or((None, &[][..]), |(l, m)| (Some(l), m));
    let plain =
        |s: &str| !s.is_empty() && s != "." && s != ".." && !s.contains('%') && !s.contains('\\');
    if !middle.iter().all(|s| plain(s)) || last.is_some_and(|s| !s.is_empty() && !plain(s)) {
        return rest_error(Error::NotFound);
    }
    let mut location = format!("/{owner}/{repo}/pulls/{number}");
    for segment in rest {
        location = format!("{location}/{segment}");
    }
    if let Some(query) = uri.query() {
        location = format!("{location}?{query}");
    }
    (
        StatusCode::MOVED_PERMANENTLY,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response()
}

/// gh's socket setting sends every host's requests here, each with that host's token, so a request
/// for a host not served is refused before anything else happens.
async fn select_host(State(state): State<AppState>, mut req: HttpRequest, next: Next) -> Response {
    let Some(host) = state
        .hosts
        .iter()
        .find(|host| addressed_to(&host.name, &req) || addressed_to(&host.public_url, &req))
        .cloned()
    else {
        let served: Vec<_> = state
            .hosts
            .iter()
            .filter_map(|host| host.name.host_str())
            .collect();
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(json!({"message": format!("This gateway serves only {}", served.join(", "))})),
        )
            .into_response();
    };
    req.extensions_mut().insert(host);
    next.run(req).await
}

/// A token not meant for the request's host is refused before it is used.
async fn authenticate(
    Extension(host): Extension<Arc<Host>>,
    mut req: HttpRequest,
    next: Next,
) -> Response {
    let auth = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let result = if host.admits(auth) {
        match &host.upstream_token {
            Some(token) => host.forgejo.authenticate(&format!("token {token}")).await,
            None => host.forgejo.authenticate(auth).await,
        }
    } else {
        Err(Error::Unauthorized)
    };
    match result {
        Ok(session) => {
            req.extensions_mut().insert(session);
            next.run(req).await
        }
        // GitHub answers bad credentials with 401 on GraphQL too (Opus o2#1).
        Err(error @ Error::Unauthorized) => rest_error(error),
        Err(error) if req.uri().path() == "/api/graphql" => {
            let error = graphql::error(error);
            Json(json!({"errors": [{"message": error.message, "type": error.extensions.unwrap().get("type"), "path": []}]})).into_response()
        }
        Err(error) => rest_error(error),
    }
}

/// Whether every authority the request names (its target's and each `Host`) is the public URL's
/// host and port, and it names at least one.
fn addressed_to(public: &Url, req: &HttpRequest) -> bool {
    let (Some(name), Some(port)) = (public.host(), public.port_or_known_default()) else {
        return false;
    };
    // A missing port is the default for the scheme the client used, not the configured port.
    let default = match req.uri().scheme_str().unwrap_or(public.scheme()) {
        "http" => 80,
        "https" => 443,
        _ => return false,
    };
    let name = name.to_owned();
    let target = req.uri().authority().map(|a| a.as_str().as_bytes());
    let headers = req.headers().get_all(HOST).iter().map(|v| v.as_bytes());
    let mut named = false;
    for authority in target.into_iter().chain(headers) {
        named = true;
        if !names(authority, &name, port, default) {
            return false;
        }
    }
    named
}

/// Case doesn't matter, and `[::1]` names `[0:0:0:0:0:0:0:1]`.
/// `Host` never carries userinfo (RFC 9110 §7.2), and an explicit port must be a real one.
fn names(authority: &[u8], name: &url::Host, port: u16, default: u16) -> bool {
    let Ok(authority) = Authority::try_from(authority) else {
        return false;
    };
    let Some(rest) = authority.as_str().strip_prefix(authority.host()) else {
        return false;
    };
    let explicit = match rest.strip_prefix(':') {
        None if rest.is_empty() => default,
        Some("") => default,
        Some(digits) if digits.bytes().all(|b| b.is_ascii_digit()) => match digits.parse() {
            Ok(explicit) => explicit,
            Err(_) => return false,
        },
        _ => return false,
    };
    // `url` reads IPv4 the way browsers do (`127.1`, `2130706433`), but gh keys tokens by host
    // text, so only the spelling `url` writes back names a host; IPv6 compares as a value.
    let text = authority.host();
    explicit == port
        && url::Host::parse(text).is_ok_and(|parsed| {
            (matches!(parsed, url::Host::Ipv6(_)) || parsed.to_string().eq_ignore_ascii_case(text))
                && parsed == *name
        })
}

async fn graphql_request(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Extension(host): Extension<Arc<Host>>,
    Json(request): Json<Request>,
) -> Json<Value> {
    let response = state
        .schema
        .execute(
            request
                .data(session)
                .data(host.public_url.clone())
                .data(host.wip_prefixes.clone()),
        )
        .await;
    let mut value = serde_json::to_value(response).expect("GraphQL response is serializable");
    if let Some(errors) = value.get_mut("errors").and_then(Value::as_array_mut) {
        for error in errors {
            let kind = error
                .pointer("/extensions/type")
                .cloned()
                .unwrap_or(json!("GRAPHQL_VALIDATION_FAILED"));
            error["type"] = kind;
            if error.get("path").is_none() {
                error["path"] = json!([]);
            }
            error.as_object_mut().unwrap().remove("extensions");
        }
    }
    Json(value)
}

fn rest_error(error: Error) -> Response {
    let status = match error {
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::NotFound => StatusCode::NOT_FOUND,
        Error::Forbidden | Error::MissingScope => StatusCode::FORBIDDEN,
        Error::Timeout => StatusCode::GATEWAY_TIMEOUT,
        Error::Upstream | Error::TooLarge => StatusCode::BAD_GATEWAY,
    };
    let mut body = json!({"message": error.to_string()});
    if matches!(error, Error::Unauthorized) {
        body["documentation_url"] =
            json!("https://docs.github.com/rest/authentication/authenticating-to-the-rest-api");
    }
    (status, Json(body)).into_response()
}
