//! `launcher::env`: in-place edits and their revert (launcher spec §7). One test, run alone in
//! its own binary, because it changes this process's environment.
#![cfg(unix)]

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;

use gh_forgejo_shim::launcher::env::{apply, revert, Edit, LAUNCHER, SAVED};

fn get(name: &str) -> Option<OsString> {
    env::var_os(name)
}

#[test]
fn edits_are_recorded_and_reverted() {
    // The user's own values, one of them not UTF-8.
    env::set_var("GH_CONFIG_DIR", "/user/gh");
    env::set_var("GH_ENTERPRISE_TOKEN", OsStr::from_bytes(b"ghe-\xff"));
    env::remove_var("GH_PATH");
    env::remove_var("GIT_CONFIG_PARAMETERS");

    apply(&[
        ("GH_CONFIG_DIR", Edit::Set("/t/gfj-x".into())),
        ("GH_PATH", Edit::Set("/t/gfj-x/gh".into())),
        ("GH_ENTERPRISE_TOKEN", Edit::Remove),
        (
            "GIT_CONFIG_PARAMETERS",
            Edit::Set("'credential.helper='".into()),
        ),
    ]);
    env::set_var(LAUNCHER, "42");
    assert_eq!(get("GH_CONFIG_DIR"), Some("/t/gfj-x".into()));
    assert_eq!(get("GH_ENTERPRISE_TOKEN"), None);
    assert_eq!(
        get(SAVED),
        Some("GH_CONFIG_DIR,GH_PATH,GH_ENTERPRISE_TOKEN,GIT_CONFIG_PARAMETERS".into())
    );

    // In between: a git child appended its own -c, and something changed GH_PATH on purpose.
    env::set_var("GIT_CONFIG_PARAMETERS", "'credential.helper=' 'x.y'='z'");
    env::set_var("GH_PATH", "/elsewhere/gh");

    revert();
    assert_eq!(get("GH_CONFIG_DIR"), Some("/user/gh".into()), "restored");
    assert_eq!(
        get("GH_ENTERPRISE_TOKEN").map(|value| value.as_bytes().to_vec()),
        Some(b"ghe-\xff".to_vec()),
        "a removed variable comes back byte for byte"
    );
    assert_eq!(
        get("GH_PATH"),
        Some("/elsewhere/gh".into()),
        "a changed value stays"
    );
    assert_eq!(
        get("GIT_CONFIG_PARAMETERS"),
        Some("'x.y'='z'".into()),
        "the user had none, so only git's own entries remain, with no leading space"
    );
    for name in [
        SAVED,
        LAUNCHER,
        "FJ_SHIM_ORIG_GH_CONFIG_DIR",
        "FJ_SHIM_SET_GH_PATH",
    ] {
        assert_eq!(get(name), None, "{name} is bookkeeping");
    }

    // With a user value of its own, git's appended entries follow it after one space.
    env::set_var("GIT_CONFIG_PARAMETERS", "'a.b'='c'");
    apply(&[(
        "GIT_CONFIG_PARAMETERS",
        Edit::Set("'a.b'='c' 'credential.helper='".into()),
    )]);
    env::set_var(
        "GIT_CONFIG_PARAMETERS",
        "'a.b'='c' 'credential.helper=' 'x.y'='z'",
    );
    revert();
    assert_eq!(
        get("GIT_CONFIG_PARAMETERS"),
        Some("'a.b'='c' 'x.y'='z'".into())
    );

    // Nothing recorded: revert only drops a stray launcher pid.
    env::set_var(LAUNCHER, "7");
    env::set_var("GH_CONFIG_DIR", "/kept");
    revert();
    assert_eq!(get(LAUNCHER), None);
    assert_eq!(get("GH_CONFIG_DIR"), Some("/kept".into()));
}
