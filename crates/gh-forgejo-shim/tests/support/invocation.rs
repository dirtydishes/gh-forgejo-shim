//! Process-level routing checks, including arguments that are not valid UTF-8.

use super::support::{CliFixture, TestResult};

#[test]
fn forgejo_errors_and_unsupported_commands_never_delegate_or_need_auth() -> TestResult {
    let fixture = CliFixture::new()?;
    let marker = fixture.root().join("delegated");
    let decoy = fixture.write_executable("gh", "#!/bin/sh\n: > \"$DECOY_MARKER\"\nexit 97\n")?;
    let cases = [
        (
            vec!["pr", "checks", "1", "--watch"],
            "\"gh pr checks\" is not supported",
        ),
        (
            vec!["pr", "checks", "1", "--watch=false"],
            "\"gh pr checks\" is not supported",
        ),
        (
            vec!["pr", "checks", "1", "--fail-fast"],
            "\"gh pr checks\" is not supported",
        ),
        (
            vec!["pr", "checks", "1", "--interval", "2"],
            "\"gh pr checks\" is not supported",
        ),
        (
            vec!["pr", "checks", "1", "-i2"],
            "\"gh pr checks\" is not supported",
        ),
        (
            vec!["pr", "merge", "1", "--merge", "--delete-branch"],
            "\"gh pr merge\" is not supported",
        ),
        (
            vec!["pr", "edit", "1", "--title", "x", "--add-reviewer", "alice"],
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
fn gh_state_commands_and_help_run_plain_gh_in_a_forgejo_context() -> TestResult {
    // Launcher spec §2's delegate list: these call no API or write gh's own state, and help is
    // gh's own; real gh answers them even with GH_HOST naming an allowlisted forge.
    let fixture = CliFixture::new()?;
    let real = fixture.write_executable(
        "gh",
        "#!/bin/sh\nprintf 'real gh:'; printf ' %s' \"$@\"; echo\n",
    )?;
    for args in [
        &["auth", "login", "--hostname", "git.example.com"][..],
        &["auth", "setup-git"][..],
        &["config", "get", "editor"][..],
        &["alias", "list"][..],
        &["pr", "view", "--help"][..],
    ] {
        let output = fixture
            .command("gh-forgejo-shim")?
            .env("GH_HOST", "git.example.com")
            .env("FJ_SHIM_REAL_GH", &real)
            .arg("gh")
            .args(args)
            .output()?;
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout)?,
            format!("real gh: {}\n", args.join(" ")),
            "{args:?}"
        );
    }
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
