//! Process-level routing checks, including arguments that are not valid UTF-8.

use std::fs;

use super::support::{CliFixture, TestResult};

#[test]
fn forgejo_errors_and_unsupported_commands_never_delegate_or_need_auth() -> TestResult {
    let fixture = CliFixture::new()?;
    let marker = fixture.root().join("delegated");
    let decoy = fixture.write_executable("gh", "#!/bin/sh\n: > \"$DECOY_MARKER\"\nexit 97\n")?;
    let cases = [
        (vec!["pr", "merge", "1"], "\"gh pr merge\" is not supported"),
        (
            vec!["pr", "edit", "1", "--title", "x"],
            "\"gh pr edit\" is not supported",
        ),
        (
            vec!["api", "repos/owner/repo/pulls"],
            "\"gh api\" is not supported",
        ),
        (
            vec!["pr", "lock", "1", "--reason", "off-topic"],
            "\"gh pr lock\" is not supported",
        ),
        (
            vec!["pr", "view", "1", "--gfjs-unknown"],
            "unknown flag: --gfjs-unknown",
        ),
        (
            vec!["pr", "view", "1", "--json", "x"],
            "Unknown JSON field: \"x\"",
        ),
        (
            vec!["pr", "gfjs-unknown"],
            "unknown command \"gfjs-unknown\" for \"gh pr\"",
        ),
        (vec!["pr", "viwe"], "Did you mean this?\n\tview\n"),
    ];
    for (args, expected) in cases {
        let output = fixture
            .command("gh-forgejo-shim")?
            .env("FJ_SHIM_REAL_GH", &decoy)
            .env("DECOY_MARKER", &marker)
            .env("GH_HOST", "git.example.com")
            .arg("gh")
            .args(&args)
            .output()?;
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert!(
            String::from_utf8(output.stderr)?.contains(expected),
            "{args:?}"
        );
        assert!(!marker.exists(), "delegated {args:?}");
    }
    Ok(())
}

#[test]
fn invalid_invocation_with_a_late_attached_repo_flag_stays_local() -> TestResult {
    let fixture = CliFixture::new()?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .args([
            "gh",
            "pr",
            "view",
            "1",
            "--gfjs-unknown",
            "-Rgit.example.com/owner/repo",
        ])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)?.starts_with("unknown flag: --gfjs-unknown\n"));
    Ok(())
}

#[test]
fn auth_hostname_on_commands_missing_from_the_table_stays_local() -> TestResult {
    let fixture = CliFixture::new()?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .args(["gh", "auth", "login", "--hostname", "git.example.com"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stderr)?,
        "gh-forgejo-shim: \"gh auth login\" is not supported for Forgejo host git.example.com yet\n");
    Ok(())
}

#[test]
fn local_help_uses_the_captured_gh_help_without_auth() -> TestResult {
    let fixture = CliFixture::new()?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .env("GH_HOST", "git.example.com")
        .args(["gh", "pr", "view", "--help"])
        .output()?;
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let usage: serde_json::Value = serde_json::from_str(&fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contract/usage-errors.json"),
    )?)?;
    assert_eq!(
        String::from_utf8(output.stdout)?,
        usage["pr view"]["help"].as_str().expect("help")
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn github_delegation_keeps_every_original_argument_byte() -> TestResult {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let fixture = CliFixture::new()?;
    let decoy = fixture.write_executable("gh", "#!/bin/sh\nprintf '%s\\000' \"$@\"\nexit 23\n")?;
    let args = vec![
        OsString::from("pr"),
        OsString::from("view"),
        OsString::from("-Rgithub.com/owner/repo"),
        OsString::from("--invalid=value with spaces"),
        OsString::from_vec(vec![b'x', 0xff, b'y']),
        OsString::from("--"),
        OsString::from(""),
    ];
    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_REAL_GH", decoy)
        .env("GH_HOST", "git.example.com")
        .arg("gh")
        .args(&args)
        .output()?;
    assert_eq!(output.status.code(), Some(23));
    let expected = args
        .into_iter()
        .flat_map(|arg| arg.into_vec().into_iter().chain([0]))
        .collect::<Vec<_>>();
    assert_eq!(output.stdout, expected);
    assert!(output.stderr.is_empty());
    Ok(())
}
