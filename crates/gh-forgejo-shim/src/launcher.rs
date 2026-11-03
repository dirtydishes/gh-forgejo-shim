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
];

/// Whether a Forgejo-routed invocation goes through the launcher.
pub fn launches(invocation: &Invocation) -> bool {
    LAUNCH_LIST.contains(&invocation.command.as_str())
        // Usage errors, help and forms the built-in path refuses as unsupported keep its
        // answers, which need no token (spec §2, "Transition: the launch list").
        && invocation.validate().is_ok()
        && invocation.help().is_none()
        && invocation.is_native()
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
