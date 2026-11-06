//! The Forgejo token scopes the gateway needs, and the warning it logs when a missing one hides
//! data. The README's "Token scopes" section lists the same four.

use std::sync::{Mutex, OnceLock};

/// One scope the gateway needs. In Forgejo a `write:` scope includes its `read:` one: each write
/// bitmap carries the read bits (`models/auth/access_token_scope.go`, Forgejo v15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    /// The scope to grant, such as `write:issue`.
    pub name: &'static str,
    /// Forgejo's category, the part after the colon.
    pub category: &'static str,
    /// What goes wrong for a token without the scope, or its read half.
    pub without: &'static str,
    /// For a write scope, what goes wrong for a token with only the read half.
    pub read_only: Option<&'static str>,
}

pub const REQUIRED: [Scope; 4] = [
    Scope {
        name: "read:user",
        category: "user",
        without: "every gh call fails, since the gateway can't tell whose token it is",
        read_only: None,
    },
    Scope {
        name: "write:repository",
        category: "repository",
        without: "pull requests, branches and checks can't be read",
        read_only: Some("pull requests can't be created, edited or merged"),
    },
    Scope {
        name: "write:issue",
        category: "issue",
        without: "issues can't be read, and a pull request's comments and scheduled auto-merge \
                  read as none",
        read_only: Some("issues and comments can't be created or edited"),
    },
    Scope {
        name: "read:organization",
        category: "organization",
        without: "a repository an organization owns reads as owned by a user",
        read_only: None,
    },
];

/// The required scope in `scope`'s category, for a scope Forgejo names such as `read:issue`.
pub fn required(scope: &str) -> Option<&'static Scope> {
    let (_, category) = scope.split_once(':')?;
    REQUIRED
        .iter()
        .find(|required| required.category == category)
}

/// The scopes Forgejo's missing-scope 403 names (`token does not have at least one of required
/// scope(s): [read:issue]`, `routers/api/v1/permissions/token_requires_scopes.go`), space
/// separated. Only `read:` or `write:` and a lowercase category count, so the text that reaches a
/// log is one the gateway chose, not the upstream's.
pub(crate) fn named_in(message: &str) -> String {
    let Some((_, list)) = message.split_once("scope(s):") else {
        return String::new();
    };
    list.trim()
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .filter(|scope| {
            scope.split_once(':').is_some_and(|(level, category)| {
                matches!(level, "read" | "write")
                    && !category.is_empty()
                    && category.bytes().all(|byte| byte.is_ascii_lowercase())
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

type Sink = Box<dyn Fn(&str) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
/// The (forge, scope) pairs already warned about in this process.
static WARNED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Sends the gateway's warnings to `sink`: fga-server's log, or a launched call's trace. Without
/// one they go nowhere, since a launched gh owns standard output and error. The first call wins;
/// it returns whether this one did.
pub fn set_sink(sink: impl Fn(&str) + Send + Sync + 'static) -> bool {
    SINK.set(Box::new(sink)).is_ok()
}

/// Logs, once per process for each forge and scope, that Forgejo refused `forge`'s token for
/// missing `scope` and the gateway answered without that data.
pub(crate) fn warn_hidden(forge: &str, scope: &str) {
    {
        let mut warned = WARNED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if warned.iter().any(|(f, s)| f == forge && s == scope) {
            return;
        }
        warned.push((forge.to_owned(), scope.to_owned()));
    }
    if let Some(sink) = SINK.get() {
        sink(&warning(forge, scope));
    }
}

fn warning(forge: &str, scope: &str) -> String {
    let named = if scope.is_empty() { "a scope" } else { scope };
    match scope.split(' ').find_map(required) {
        Some(required) => format!(
            "warning: {forge}: the token lacks {named}, so {}; give it {} (README: Token scopes)",
            required.without, required.name
        ),
        None => format!(
            "warning: {forge}: the token lacks {named}, so the gateway answered without what \
             Forgejo refused (README: Token scopes)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgejo_names_the_scopes_it_wanted() {
        let message =
            |list: &str| format!("token does not have at least one of required scope(s): {list}");
        assert_eq!(named_in(&message("[read:issue]")), "read:issue");
        assert_eq!(
            named_in(&message("[read:issue read:repository]")),
            "read:issue read:repository"
        );
        // Anything else Forgejo might put there stays out of the log.
        assert_eq!(named_in(&message("[read:issue\nx]")), "read:issue");
        assert_eq!(named_in(&message("[admin:Everything secret:]")), "");
        assert_eq!(named_in("Forbidden"), "");
    }

    #[test]
    fn a_read_scope_finds_the_write_scope_that_covers_it() {
        assert_eq!(required("read:issue").map(|s| s.name), Some("write:issue"));
        assert_eq!(
            required("read:organization").map(|s| s.name),
            Some("read:organization")
        );
        assert_eq!(required("read:package"), None);
    }

    #[test]
    fn the_warning_names_the_scope_and_what_it_hides() {
        assert_eq!(
            warning("https://forge.test/", "read:issue"),
            "warning: https://forge.test/: the token lacks read:issue, so issues can't be read, \
             and a pull request's comments and scheduled auto-merge read as none; give it \
             write:issue (README: Token scopes)"
        );
        assert!(warning("https://forge.test/", "").contains("lacks a scope"));
    }
}
