//! The gateway router. It writes nothing to stdout or stderr: launched in-process, gh owns both.
#![deny(clippy::print_stdout, clippy::print_stderr)]
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
    routing::{get, post},
};
use fga_core::{
    forgejo::{Error, Forgejo, Session},
    graphql::{self, GithubSchema},
    map::WipPrefixes,
};
use serde_json::{Value, json};
use std::sync::Arc;
use url::Url;

/// One forge the gateway serves: requests addressed to `public_url` go to `forgejo`.
pub struct Host {
    pub public_url: Url,
    pub forgejo: Forgejo,
    pub wip_prefixes: WipPrefixes,
    /// When set, the only credential accepted for this host. A launcher hands each host's token
    /// over; server mode leaves it unset and lets Forgejo judge every user's token.
    expected_token: Option<String>,
}

impl Host {
    pub fn new(public_url: Url, forgejo: Forgejo, wip_prefixes: WipPrefixes) -> Self {
        Self {
            public_url,
            forgejo,
            wip_prefixes,
            expected_token: None,
        }
    }

    pub fn expecting(self, token: String) -> Self {
        Self {
            expected_token: Some(token),
            ..self
        }
    }

    /// Whether `authorization` may be used for this host at all, compared in constant time.
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
        presented.len() == expected.len()
            && presented
                .bytes()
                .zip(expected.bytes())
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
    pub fn new(hosts: Vec<Host>, ghes_version: String) -> Self {
        Self {
            hosts: hosts.into_iter().map(Arc::new).collect(),
            ghes_version,
            schema: graphql::schema(),
        }
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/api/v3/", get(rest::root))
        .route("/api/v3/meta", get(rest::meta))
        .route("/api/v3/user", get(rest::user))
        .route("/api/v3/repos/{owner}/{repo}", get(rest::repository))
        .route("/api/v3/repos/{owner}/{repo}/pulls", get(rest::pulls))
        .route(
            "/api/v3/repos/{owner}/{repo}/pulls/{number}",
            get(rest::pull),
        )
        .route("/api/v3/repos/{owner}/{repo}/readme", get(rest::readme))
        .route(
            "/api/v3/repos/{owner}/{repo}/commits/{sha}/pulls",
            get(rest::commit_pulls),
        )
        .route("/api/v3/repos/{owner}/{repo}/actions/runs", get(runs::runs))
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/runs/{id}",
            get(runs::run),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/workflows",
            get(runs::workflows),
        )
        .route(
            "/api/v3/repos/{owner}/{repo}/actions/workflows/{id}",
            get(runs::workflow),
        )
        .route("/api/graphql", post(graphql_request))
        .fallback(|| async { rest_error(Error::NotFound) })
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

async fn authenticate(State(state): State<AppState>, mut req: HttpRequest, next: Next) -> Response {
    // gh's socket setting sends every host's requests here, each with that host's token, so a
    // request for a host not served, or with a token not meant for it, is refused before the
    // token is used.
    let Some(host) = state
        .hosts
        .iter()
        .find(|host| addressed_to(&host.public_url, &req))
        .cloned()
    else {
        let served: Vec<_> = state
            .hosts
            .iter()
            .filter_map(|host| host.public_url.host_str())
            .collect();
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(json!({"message": format!("This gateway serves only {}", served.join(", "))})),
        )
            .into_response();
    };
    let auth = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let result = if host.admits(auth) {
        host.forgejo.authenticate(auth).await
    } else {
        Err(Error::Unauthorized)
    };
    match result {
        Ok(session) => {
            req.extensions_mut().insert(session);
            req.extensions_mut().insert(host);
            next.run(req).await
        }
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
    let (Some(host), Some(port)) = (public.host(), public.port_or_known_default()) else {
        return false;
    };
    // A missing port is the default for the scheme the client used, not the configured port.
    let default = match req.uri().scheme_str().unwrap_or(public.scheme()) {
        "http" => 80,
        "https" => 443,
        _ => return false,
    };
    let host = host.to_owned();
    let target = req.uri().authority().map(|a| a.as_str().as_bytes());
    let headers = req.headers().get_all(HOST).iter().map(|v| v.as_bytes());
    let mut named = false;
    for authority in target.into_iter().chain(headers) {
        named = true;
        if !names(authority, &host, port, default) {
            return false;
        }
    }
    named
}

/// Case doesn't matter, and `[::1]` names `[0:0:0:0:0:0:0:1]`.
/// `Host` never carries userinfo (RFC 9110 §7.2), and an explicit port must be a real one.
fn names(authority: &[u8], host: &url::Host, port: u16, default: u16) -> bool {
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
                && parsed == *host
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
        Error::Forbidden => StatusCode::FORBIDDEN,
        Error::Timeout => StatusCode::GATEWAY_TIMEOUT,
        Error::Upstream => StatusCode::BAD_GATEWAY,
    };
    let mut body = json!({"message": error.to_string()});
    if matches!(error, Error::Unauthorized) {
        body["documentation_url"] =
            json!("https://docs.github.com/rest/authentication/authenticating-to-the-rest-api");
    }
    (status, Json(body)).into_response()
}
