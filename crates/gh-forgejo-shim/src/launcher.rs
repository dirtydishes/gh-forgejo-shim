//! The launcher: real gh against a Forgejo host through an in-process gateway on a private unix
//! socket. The design is `docs/rewrite/launcher.md`; section numbers below refer to it.

use std::collections::HashMap;

use crate::invocation::Invocation;

pub mod browser;
#[cfg(unix)]
pub mod credentials;
#[cfg(unix)]
pub mod env;
#[cfg(unix)]
pub mod gateway;
pub mod ghconfig;
#[cfg(unix)]
pub mod run;
#[cfg(unix)]
pub mod rundir;
#[cfg(unix)]
pub mod start;
pub mod upstream;

/// The commands the launcher runs through real gh (spec §2, "Transition: the launch list"),
/// matched on the command after alias expansion. Every other Forgejo route keeps the built-in
/// path. A command joins in the change that makes the gateway serve it, with its test; C5
/// deletes the built-in path, and this list, once it covers everything.
pub const LAUNCH_LIST: &[&str] = &[
    "pr view",
    "pr checks",
    "pr list",
    "api",
    "auth token",
    "auth status",
    "pr create",
    "pr comment",
    "pr edit",
    "pr merge",
    "pr ready",
    "pr close",
    "pr reopen",
    "pr checkout",
    "run list",
    "run view",
    "issue comment",
];

/// Listed commands that launch only with the flags a trace showed the gateway serves (C3a,
/// `cwe.15.1`; C3b, `cwe.16.1`; C4, `cwe.18.1`); `repo` is gh's `-R`. Any other flag keeps the built-in path,
/// which serves or refuses that form before anything is written: gh would write part of a
/// `pr edit` and then fail on a reviewer the gateway can't set. `--delete-branch` stays off: the
/// gateway can't delete a branch yet, and gh would report success with the branch still there.
/// For these commands the list replaces `is_native()`, which can't see the built-in path's
/// per-flag refusals.
const LAUNCH_FLAGS: &[(&str, &[&str])] = &[
    (
        "pr create",
        &["head", "base", "title", "body", "draft", "repo"],
    ),
    ("pr comment", &["body", "repo"]),
    ("pr edit", &["title", "body", "repo"]),
    (
        "pr merge",
        &[
            "merge",
            "squash",
            "rebase",
            "auto",
            "match-head-commit",
            "repo",
        ],
    ),
    ("pr ready", &["undo", "repo"]),
    ("pr close", &["repo"]),
    ("pr reopen", &["repo"]),
    // Git-side flags only; the e2e run checks `--branch`, `--detach`, `--force` and `-R` for a
    // same-repository pull request (C3c, `cwe.17`). `--recurse-submodules` and `--worktree` stay
    // off until traced.
    ("pr checkout", &["branch", "detach", "force", "repo"]),
    // `--workflow`, `--user`, `--created` and `--attempt` fail on the gateway. `--log`,
    // `--log-failed` and `--exit-status` matter only for a completed run, which no trace has
    // covered; `--verbose` shows jobs, which the gateway can't list (C4, `cwe.18.1`).
    (
        "run list",
        &[
            "json", "jq", "limit", "status", "branch", "event", "commit", "all", "repo",
        ],
    ),
    ("run view", &["json", "jq", "repo"]),
    // `--edit-last`, `--delete-last` and `--create-if-none` need mutations the gateway lacks.
    ("issue comment", &["body", "body-file", "repo"]),
];

/// `gh api` reads the gateway serves beyond the built-in path's `user`: the rate-limit overview
/// (C1, `cwe.11`), a GET with the flags traced for it.
fn reads_rate_limit(invocation: &Invocation) -> bool {
    invocation.command == "api"
        && matches!(
            invocation.positionals.as_slice(),
            [path] if path == "rate_limit" || path == "/rate_limit"
        )
        && invocation
            .flags
            .keys()
            .all(|flag| ["jq", "include", "hostname", "method"].contains(&flag.as_str()))
        && invocation
            .flag("method")
            .is_none_or(|method| method.eq_ignore_ascii_case("GET"))
}

/// Whether a Forgejo-routed invocation goes through the launcher.
pub fn launches(invocation: &Invocation) -> bool {
    let command = invocation.command.as_str();
    let served = match LAUNCH_FLAGS.iter().find(|(listed, _)| *listed == command) {
        Some((_, flags)) => invocation
            .flags
            .keys()
            .all(|flag| flags.contains(&flag.as_str())),
        None => invocation.is_native() || reads_rate_limit(invocation),
    };
    // Without a method, gh's terminal prompts ask whether to delete the branch, and to edit the
    // commit message: the forms kept off above. One method skips both prompts (`merge.go:307-330`).
    let one_method = command != "pr merge"
        || ["merge", "squash", "rebase"]
            .iter()
            // gh counts a method by its value, so `--merge=false` is none (`merge.go:104-118`).
            .filter(|method| invocation.flag(method) == Some("true"))
            .count()
            == 1;
    LAUNCH_LIST.contains(&command)
        && one_method
        // Usage errors and forms the built-in path refuses as unsupported keep its answers,
        // which need no token (spec §2, "Transition: the launch list"). Help never gets here:
        // `routing::run_gh` hands it to plain gh first; the check keeps `launches` alone from
        // ever launching a help form.
        && invocation.validate().is_ok()
        && invocation.help().is_none()
        && served
        // Without a run ID, gh prompts for one in a terminal, which no trace has covered. Without
        // `--json`, gh prints the run's jobs, which the gateway can't list, and for a failed run
        // with none blames the workflow file (o2#1).
        && !(command == "run view"
            && (invocation.positionals.is_empty() || invocation.json_fields.is_none()))
        // The gateway can't fill these yet: a run's jobs (Forgejo 15's API lists none) and its
        // attempt (it reads 0; GitHub's first is 1). gh would print them empty and exit 0.
        && !(matches!(command, "run list" | "run view")
            && invocation.json_fields.as_ref().is_some_and(|fields| {
                fields.iter().any(|field| field == "jobs" || field == "attempt")
            }))
        // Without a body, gh asks in a terminal whether to write the comment in an editor or a
        // browser, the forms kept off above (`pr/shared/commentable.go`).
        && !(matches!(command, "pr comment" | "issue comment")
            && !invocation.flags.contains_key("body")
            && !invocation.flags.contains_key("body-file"))
        // gh opens `pr checks --web` at `/pull/N/checks`, which Forgejo has no page for; the
        // built-in path prints the checks instead.
        && !(invocation.command == "pr checks" && invocation.flags.contains_key("web"))
        // gh reads a PR URL only as `/pull/N`, and Forgejo's own URLs say `/pulls/N`; a URL
        // selector keeps the built-in path, which reads both, until the launcher rewrites it.
        && !invocation
            .positionals
            .iter()
            .any(|positional| positional.contains("://"))
}

/// Guard 3's cap on nested wrapper invocations (§8).
pub const MAX_DEPTH: u32 = 8;

/// `FJ_SHIM_DEPTH` as this invocation inherited it.
pub fn depth(env: &HashMap<String, String>) -> u32 {
    env.get("FJ_SHIM_DEPTH")
        .and_then(|depth| depth.parse().ok())
        .unwrap_or(0)
}

/// Guards 2 and 3 (§8), run before anything else in the wrapper. Returns the exit code when one
/// trips: the launcher spawned this process as real gh, or nested calls went too deep.
pub fn recursion_guard(env: &HashMap<String, String>) -> Option<i32> {
    #[cfg(unix)]
    if env
        .get("FJ_SHIM_LAUNCHER")
        .and_then(|pid| pid.parse::<u32>().ok())
        .is_some_and(|pid| pid == std::os::unix::process::parent_id())
    {
        let path = std::env::current_exe().unwrap_or_default();
        eprintln!(
            "gh-forgejo-shim: the real gh at {} is gh-forgejo-shim; set FJ_SHIM_REAL_GH",
            path.display()
        );
        return Some(127);
    }
    if depth(env) >= MAX_DEPTH {
        eprintln!(
            "gh-forgejo-shim: gh resolves back to gh-forgejo-shim (depth {MAX_DEPTH}); set FJ_SHIM_REAL_GH"
        );
        return Some(127);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(args: &[&str]) -> Invocation {
        Invocation::parse(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn only_listed_commands_launch() {
        assert!(launches(&invocation(&["pr", "view", "12"])));
        assert!(launches(&invocation(&[
            "pr", "checks", "12", "--json", "name"
        ])));
        assert!(launches(&invocation(&["pr", "list", "--json", "number"])));
        assert!(launches(&invocation(&[
            "pr", "create", "--title", "t", "--body", "b"
        ])));
        assert!(launches(&invocation(&[
            "pr", "comment", "12", "--body", "b"
        ])));
        for args in [
            &["pr", "merge", "12", "--merge"][..],
            &["pr", "merge", "12", "-s", "--auto"][..],
            &["pr", "merge", "12", "-r", "--match-head-commit", "abc"][..],
            &["pr", "ready", "12", "--undo"][..],
            &["pr", "close", "12"][..],
            &["pr", "reopen", "12"][..],
            &["pr", "checkout", "12"][..],
            &["pr", "checkout", "12", "-b", "local", "--force"][..],
            &["pr", "checkout", "12", "--detach"][..],
            &["run", "list"][..],
            &[
                "run", "list", "--json", "status", "--limit", "5", "-b", "main",
            ][..],
            &[
                "run", "list", "--status", "queued", "--event", "push", "--all",
            ][..],
            &[
                "run",
                "view",
                "7",
                "--json",
                "status,conclusion",
                "--jq",
                ".status",
            ][..],
            &["issue", "comment", "3", "--body", "b"][..],
            &["issue", "comment", "3", "-F", "notes.md"][..],
            &["api", "rate_limit"][..],
            &["api", "/rate_limit", "--jq", ".rate", "-i", "-X", "GET"][..],
        ] {
            assert!(launches(&invocation(args)), "{args:?}");
        }
        assert!(launches(&invocation(&[
            "pr", "edit", "12", "-t", "t", "-b", "b"
        ])));
        assert!(launches(&invocation(&[
            "pr", "create", "-H", "x", "-B", "main", "-t", "t", "-b", "b", "-d"
        ])));
        // Flags the gateway doesn't serve keep the built-in path, which refuses them before
        // writing: gh would write the title, then fail on the reviewer.
        for refused in [
            &[
                "pr",
                "edit",
                "12",
                "--title",
                "t",
                "--add-reviewer",
                "alice",
            ][..],
            &["pr", "edit", "12", "--add-label", "bug"][..],
            &[
                "pr", "create", "--title", "t", "--body", "b", "--label", "bug",
            ][..],
            &[
                "pr",
                "create",
                "--title",
                "t",
                "--body",
                "b",
                "--reviewer",
                "alice",
            ][..],
            &["pr", "comment", "12", "--edit-last", "--body", "b"][..],
            &["pr", "comment", "12", "--delete-last", "--yes"][..],
            // The gateway can't delete a branch yet; gh would report success anyway.
            &["pr", "merge", "12", "--squash", "--delete-branch"][..],
            &["pr", "close", "12", "--delete-branch"][..],
            &["pr", "merge", "12", "--merge", "--subject", "s"][..],
            &["pr", "close", "12", "--comment", "c"][..],
            // No method: in a terminal gh would ask to delete the branch.
            &["pr", "merge", "12"][..],
            &["pr", "merge", "12", "--auto"][..],
            &["pr", "merge", "12", "--merge", "--squash"][..],
            &["pr", "merge", "12", "--merge=false"][..],
            &["pr", "merge", "12", "--auto", "--squash=false"][..],
            &["pr", "merge", "12", "--merge", "--merge=false"][..],
            &["pr", "checkout", "12", "--recurse-submodules"][..],
            &["pr", "checkout", "12", "--worktree", "../w"][..],
            // Untraced or failing on the gateway.
            &["run", "list", "--workflow", "ci.yml"][..],
            &["run", "list", "--user", "alice"][..],
            &["run", "view", "7", "--attempt", "1"][..],
            &["run", "view", "7", "--log"][..],
            // Only for a completed run, or the jobs the gateway can't list.
            &["run", "view", "7"][..],
            &["run", "view", "7", "--exit-status"][..],
            &["run", "view", "7", "--verbose"][..],
            &["run", "view", "7", "--json", "status,jobs"][..],
            &["run", "view", "7", "--json", "attempt"][..],
            &["run", "list", "--json", "databaseId,attempt"][..],
            // gh would ask for an editor or a browser in a terminal.
            &["issue", "comment", "3"][..],
            &["pr", "comment", "12"][..],
            // gh would prompt for a run in a terminal.
            &["run", "view"][..],
            &["issue", "comment", "3", "--edit-last", "--body", "b"][..],
            &["issue", "comment", "3", "--delete-last", "--yes"][..],
            &["api", "rate_limit", "-X", "POST"][..],
            &["api", "rate_limit", "--paginate"][..],
            &["api", "repos/o/r/pulls"][..],
        ] {
            assert!(!launches(&invocation(refused)), "{refused:?}");
        }
        assert!(!launches(&invocation(&["issue", "list"])));
        assert!(!launches(&invocation(&["issue", "view", "3"])));
        assert!(
            launches(&invocation(&["pr", "view", "12", "--web"])),
            "the browser helper"
        );
        assert!(!launches(&invocation(&["pr", "checks", "12", "--web"])));
        assert!(!launches(&invocation(&["pr", "view", "--help"])));
        assert!(!launches(&invocation(&[
            "pr",
            "view",
            "1",
            "--gfjs-unknown"
        ])));
        assert!(!launches(&invocation(&["pr", "checks", "1", "--watch"])));
        assert!(!launches(&invocation(&[
            "pr",
            "view",
            "https://forge.test/o/r/pulls/1"
        ])));
        assert!(!launches(&invocation(&[
            "pr",
            "checks",
            "https://forge.test/o/r/pull/1"
        ])));
    }

    #[test]
    fn the_contract_runner_mirrors_the_launch_list() {
        let mirror = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contract/launch-list.txt"
        ));
        let listed: Vec<&str> = mirror
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert_eq!(
            listed, LAUNCH_LIST,
            "update contract/launch-list.txt with LAUNCH_LIST"
        );
    }

    #[test]
    fn depth_cap_trips_at_eight() {
        let env = |depth: &str| HashMap::from([("FJ_SHIM_DEPTH".to_string(), depth.to_string())]);
        assert_eq!(depth(&env("7")), 7);
        assert_eq!(recursion_guard(&env("7")), None);
        assert_eq!(recursion_guard(&env("8")), Some(127));
        assert_eq!(
            recursion_guard(&env("junk")),
            None,
            "an unreadable depth counts as 0"
        );
    }

    #[test]
    fn a_launcher_pid_that_is_not_the_parent_passes() {
        // u32::MAX is never a real pid, so never this test's parent, even when cargo runs as a
        // container's pid 1.
        let env = HashMap::from([("FJ_SHIM_LAUNCHER".to_string(), u32::MAX.to_string())]);
        assert_eq!(recursion_guard(&env), None);
    }
}
