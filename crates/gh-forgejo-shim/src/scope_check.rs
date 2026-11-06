//! `doctor`'s check of a host's token scopes, against the four in [`fga_core::scopes::REQUIRED`].
//!
//! Forgejo checks a route's scope before it looks up the owner or repository the route names
//! (`routers/api/v1/api.go`: `tokenRequiresScopes` is the `/repos` and `/orgs/{org}` groups'
//! middleware, `repoAssignment` and `orgAssignment` the inner groups', and outer middleware runs
//! first, `modules/web/route.go`). So a request for an owner that can't exist answers 404 when the
//! token has the scope and Forgejo's missing-scope 403 when it doesn't, and reads or changes
//! nothing. Forgejo reads a POST as needing the `write:` scope (`requiredScopeLevel`), which makes
//! write scopes checkable the same way: the request fails at the lookup either way.

use reqwest::Method;
use serde_json::Value;

use crate::forgejo::ForgejoClient;
use fga_core::scopes::{Scope, REQUIRED};

/// An owner and repository name Forgejo never allows: two separators in a row
/// (`modules/validation/helpers.go`, `invalidUsernamePattern`).
const NOWHERE: &str = "gfj--scope-probe";

/// What one request says about a scope.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Answer {
    Granted,
    Missing,
    /// Neither of the two answers above, with why, so the scope can't be judged.
    Unknown(String),
}

/// What a host's token has of one required scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    Granted,
    Missing,
    /// A write scope's read half only.
    ReadOnly,
    Unknown(String),
}

/// The result of checking one host's token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Forgejo rejected the token outright (401).
    Rejected,
    /// The first request got no usable answer, so nothing was checked.
    Unchecked(String),
    Checked(Vec<(&'static Scope, Standing)>),
}

/// Checks `host`'s token against every required scope.
pub fn check(client: &ForgejoClient, host: &str) -> Outcome {
    let user = match client.probe(host, Method::GET, "/api/v1/user") {
        Err(error) => return Outcome::Unchecked(error.to_string()),
        Ok((401, _)) => return Outcome::Rejected,
        Ok((200, _)) => Standing::Granted,
        Ok((status, body)) => match refusal(status, &body) {
            Some(Answer::Missing) => Standing::Missing,
            _ => return Outcome::Unchecked(format!("Forgejo answered HTTP {status}")),
        },
    };
    let ask = |method: Method, path: String| match client.probe(host, method, &path) {
        Err(error) => Answer::Unknown(error.to_string()),
        Ok((status, body)) => refusal(status, &body)
            .unwrap_or_else(|| Answer::Unknown(format!("Forgejo answered HTTP {status}"))),
    };
    let repo = format!("/api/v1/repos/{NOWHERE}/{NOWHERE}");
    let mut results = Vec::new();
    for scope in &REQUIRED {
        let standing = match scope.category {
            "user" => user.clone(),
            "organization" => standing(ask(Method::GET, format!("/api/v1/orgs/{NOWHERE}"))),
            "repository" => write_standing(ask(Method::POST, format!("{repo}/pulls")), || {
                ask(Method::GET, repo.clone())
            }),
            "issue" => write_standing(ask(Method::POST, format!("{repo}/issues")), || {
                ask(Method::GET, format!("{repo}/issues"))
            }),
            _ => Standing::Unknown("doctor has no check for it".to_string()),
        };
        results.push((scope, standing));
    }
    Outcome::Checked(results)
}

/// How Forgejo answered a request for [`NOWHERE`]: its lookup's 404 means the scope is there, its
/// missing-scope 403 that it isn't. Anything else, a proxy's 404 or another 403 included, is
/// neither.
fn refusal(status: u16, body: &Value) -> Option<Answer> {
    let message = body["message"].as_str().unwrap_or_default();
    match status {
        404 if matches!(message, "GetUserByName" | "GetOrgByName") => Some(Answer::Granted),
        403 if message.starts_with("token does not have at least one of required scope(s)") => {
            Some(Answer::Missing)
        }
        _ => None,
    }
}

fn standing(answer: Answer) -> Standing {
    match answer {
        Answer::Granted => Standing::Granted,
        Answer::Missing => Standing::Missing,
        Answer::Unknown(why) => Standing::Unknown(why),
    }
}

/// A write scope from its write request, asking about the read half only when the write is
/// missing.
fn write_standing(write: Answer, read: impl FnOnce() -> Answer) -> Standing {
    match write {
        Answer::Granted => Standing::Granted,
        Answer::Unknown(why) => Standing::Unknown(why),
        Answer::Missing => match read() {
            Answer::Granted => Standing::ReadOnly,
            _ => Standing::Missing,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_forgejos_own_answers_count() {
        let scope = json!({"message":"token does not have at least one of required scope(s): [write:issue]"});
        assert_eq!(refusal(403, &scope), Some(Answer::Missing));
        assert_eq!(
            refusal(404, &json!({"message":"GetUserByName"})),
            Some(Answer::Granted)
        );
        assert_eq!(
            refusal(404, &json!({"message":"GetOrgByName"})),
            Some(Answer::Granted)
        );
        // A proxy's 404, another refusal, a route this Forgejo lacks: none says either way.
        assert_eq!(refusal(404, &Value::Null), None);
        assert_eq!(refusal(404, &json!({"message":"Not Found"})), None);
        assert_eq!(refusal(403, &json!({"message":"forbidden"})), None);
        assert_eq!(refusal(405, &scope), None);
    }

    #[test]
    fn a_write_scope_without_its_write_half_may_still_read() {
        assert_eq!(
            write_standing(Answer::Missing, || Answer::Granted),
            Standing::ReadOnly
        );
        assert_eq!(
            write_standing(Answer::Missing, || Answer::Missing),
            Standing::Missing
        );
        assert_eq!(
            write_standing(Answer::Granted, || unreachable!("no read asked")),
            Standing::Granted
        );
    }
}
