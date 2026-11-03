//! The wrapper's own environment, edited in place (launcher spec §7).
//!
//! Real gh, launched or delegated to, inherits this process's environment untouched: a
//! `Command` with no environment calls passes `environ` through byte for byte, entries without
//! `=`, repeated names and order included. Any `Command::env` call would instead rebuild it from
//! `vars_os()` into a sorted map. So the wrapper changes only the variables it names, here, with
//! `set_var` and `remove_var`, and only while it is single-threaded: at entry, and on the launch
//! path before the gateway's runtime starts.
//!
//! Each change is recorded so a nested call can put the user's value back: `FJ_SHIM_SAVED` lists
//! the names, `FJ_SHIM_ORIG_<NAME>` holds a value the user had, and `FJ_SHIM_SET_<NAME>` the
//! value the launcher set.

use std::env;
use std::ffi::{OsStr, OsString};

pub const SAVED: &str = "FJ_SHIM_SAVED";
pub const DEPTH: &str = "FJ_SHIM_DEPTH";
pub const LAUNCHER: &str = "FJ_SHIM_LAUNCHER";
const ORIG: &str = "FJ_SHIM_ORIG_";
const SET: &str = "FJ_SHIM_SET_";
const GIT_CONFIG_PARAMETERS: &str = "GIT_CONFIG_PARAMETERS";

/// One change to a named variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Set(OsString),
    Remove,
}

/// Applies `edits` to this process's environment and records each for [`revert`]. Call only
/// while no other thread can read the environment.
pub fn apply(edits: &[(&str, Edit)]) {
    let mut saved: Vec<&str> = Vec::new();
    for (name, edit) in edits {
        // The user's value is the one before the first edit of a name, not a later one.
        let first = !saved.contains(name);
        let original = env::var_os(name);
        match edit {
            Edit::Set(value) => {
                set_once(name, value);
                set_once(&format!("{SET}{name}"), value);
            }
            Edit::Remove => {
                env::remove_var(name);
                env::remove_var(format!("{SET}{name}"));
            }
        }
        if first {
            match original {
                Some(value) => set_once(&format!("{ORIG}{name}"), &value),
                None => env::remove_var(format!("{ORIG}{name}")),
            }
            saved.push(name);
        }
    }
    if !saved.is_empty() {
        env::set_var(SAVED, saved.join(","));
    }
}

/// Puts back every variable a launcher changed, unless something in between changed it on
/// purpose, then removes the bookkeeping, except `FJ_SHIM_DEPTH` (guard 3 needs it). Runs at
/// wrapper entry, after guard 2, while single-threaded.
pub fn revert() {
    let Some(saved) = env::var_os(SAVED) else {
        env::remove_var(LAUNCHER);
        return;
    };
    for name in saved
        .to_string_lossy()
        .split(',')
        .filter(|name| !name.is_empty())
    {
        let orig_name = format!("{ORIG}{name}");
        let set_name = format!("{SET}{name}");
        let original = env::var_os(&orig_name);
        let set = env::var_os(&set_name);
        let current = env::var_os(name);
        if current == set {
            // Unchanged since the launcher: the user's value, or none.
            match &original {
                Some(value) => set_once(name, value),
                None => env::remove_var(name),
            }
        } else if name == GIT_CONFIG_PARAMETERS {
            // A git that runs a child with `-c` appends a space and its own entries.
            if let (Some(current), Some(set)) = (&current, &set) {
                if let Some(rest) = strip_prefix_space(current, set) {
                    let value = match original.as_deref().filter(|value| !value.is_empty()) {
                        Some(original) => join_space(original, rest),
                        None => rest.to_os_string(),
                    };
                    if value.is_empty() {
                        env::remove_var(name);
                    } else {
                        set_once(name, &value);
                    }
                }
            }
        }
        env::remove_var(&orig_name);
        env::remove_var(&set_name);
    }
    env::remove_var(SAVED);
    env::remove_var(LAUNCHER);
}

/// `value` without `prefix` and the space after it, if it starts with both.
fn strip_prefix_space<'a>(value: &'a OsStr, prefix: &OsStr) -> Option<&'a OsStr> {
    use std::os::unix::ffi::OsStrExt;
    let rest = value
        .as_bytes()
        .strip_prefix(prefix.as_bytes())?
        .strip_prefix(b" ")?;
    Some(OsStr::from_bytes(rest))
}

fn join_space(first: &OsStr, second: &OsStr) -> OsString {
    let mut joined = first.to_os_string();
    joined.push(" ");
    joined.push(second);
    joined
}

/// `FJ_SHIM_DEPTH` plus one, for everything this invocation starts (§8, guard 3).
pub fn increment_depth(depth: u32) {
    set_once(DEPTH, OsStr::new(&(depth + 1).to_string()));
}

/// Sets `name` to `value` as the only entry with that name. `set_var` changes only the first of
/// repeated entries, while a reader may take the last, so a stale duplicate could hide the new
/// value: `FJ_SHIM_DEPTH` twice would never reach guard 3's cap. `remove_var` removes every
/// entry with the name first.
fn set_once(name: &str, value: &OsStr) {
    env::remove_var(name);
    env::set_var(name, value);
}
