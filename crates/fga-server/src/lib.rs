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
use url::Url;

#[derive(Clone)]
pub struct AppState {
    pub forgejo: Forgejo,
    pub public_url: Url,
    pub ghes_version: String,
    pub wip_prefixes: WipPrefixes,
    schema: GithubSchema,
}

impl AppState {
    pub fn new(
        forgejo: Forgejo,
        public_url: Url,
        ghes_version: String,
        wip_prefixes: WipPrefixes,
    ) -> Self {
        Self {
            forgejo,
            public_url,
            ghes_version,
            wip_prefixes,
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
    // request for another host is refused before its token is used.
    if !addressed_to(&state.public_url, &req) {
        let host = state.public_url.host_str().unwrap_or_default();
        return (
            StatusCode::MISDIRECTED_REQUEST,
            Json(json!({"message": format!("This gateway serves only {host}")})),
        )
            .into_response();
    }
    let auth = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    match state.forgejo.authenticate(auth).await {
        Ok(session) => {
            req.extensions_mut().insert(session);
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
/// host and port, and it names at least one. A missing port is the public URL's default port.
fn addressed_to(public: &Url, req: &HttpRequest) -> bool {
    let (Some(host), Some(port)) = (public.host_str(), public.port_or_known_default()) else {
        return false;
    };
    let target = req.uri().authority().map(|a| a.as_str().as_bytes());
    let headers = req.headers().get_all(HOST).iter().map(|v| v.as_bytes());
    let mut named = false;
    for authority in target.into_iter().chain(headers) {
        named = true;
        let Ok(authority) = Authority::try_from(authority) else {
            return false;
        };
        if !authority.host().eq_ignore_ascii_case(host)
            || authority.port_u16().unwrap_or(port) != port
        {
            return false;
        }
    }
    named
}

async fn graphql_request(
    State(state): State<AppState>,
    Extension(session): Extension<Session>,
    Json(request): Json<Request>,
) -> Json<Value> {
    let response = state
        .schema
        .execute(
            request
                .data(session)
                .data(state.public_url)
                .data(state.wip_prefixes),
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
