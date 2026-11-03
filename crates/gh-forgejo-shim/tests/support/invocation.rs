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
        // -h is --homepage, and --help a value of --notes: writes, not help, so not plain gh.
        (
            vec!["repo", "edit", "-h", "https://x.test"],
            "\"gh repo edit\" is not supported",
        ),
        (
            vec!["release", "create", "v1", "--notes", "--help"],
            "\"gh release\" is not supported",
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
fn github_target_in_doubt_near_a_forge_is_refused() -> TestResult {
    // Where the parse may not read the call as gh does (a flag before the command word, a command
    // the grammar doesn't cover, a call it rejects), plain gh runs only with no Forgejo host in
    // reach (Sol r3#1 on #114).
    let fixture = CliFixture::new()?;
    let decoy = fixture.write_executable("gh", "#!/bin/sh\necho ran\nexit 23\n")?;
    let forge_repo = [("GH_REPO", "git.example.com/o/r")];
    for (args, env) in [
        // gh runs `release create` with notes `api`, in GH_REPO.
        (
            &["--notes", "api", "release", "create", "v1"][..],
            &[
                ("GH_REPO", "git.example.com/o/r"),
                ("GH_HOST", "github.com"),
            ][..],
        ),
        (
            &[
                "--notes",
                "api",
                "--title",
                "https://github.com",
                "release",
                "create",
                "v1",
            ][..],
            &forge_repo[..],
        ),
        // `-R=…` is `--notes`'s value to gh.
        (
            &["--notes", "-R=github.com/x/y", "release", "create", "v1"][..],
            &forge_repo[..],
        ),
        // No grammar for `release create`: `--notes` takes `-R` as its value.
        (
            &["release", "create", "v1", "--notes", "-R", "github.com/x/y"][..],
            &forge_repo[..],
        ),
        // The Forgejo host in an argument the parse may have misread.
        (
            &[
                "release",
                "create",
                "v1",
                "--notes",
                "x",
                "-R",
                "git.example.com/o/r",
            ][..],
            &[][..],
        ),
        // A call the grammar rejects: a newer gh may have the flag.
        (
            &["pr", "view", "-Rgithub.com/owner/repo", "--invalid=x"][..],
            &[("GH_HOST", "git.example.com")][..],
        ),
    ] {
        let output = fixture
            .command("gh-forgejo-shim")?
            .env("FJ_SHIM_REAL_GH", &decoy)
            .envs(env.iter().copied())
            .arg("gh")
            .args(args)
            .output()?;
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: plain gh ran");
    }
    // Read as gh reads it, or nothing Forgejo in reach: plain gh runs.
    for (args, env) in [
        (
            &["pr", "view", "1", "-R", "github.com/x/y"][..],
            &[("GH_HOST", "git.example.com")][..],
        ),
        (
            &["release", "create", "v1", "--repo", "github.com/x/y"][..],
            &[][..],
        ),
        (&["--notes", "api", "release", "create", "v1"][..], &[][..]),
    ] {
        let output = fixture
            .command("gh-forgejo-shim")?
            .env("FJ_SHIM_REAL_GH", &decoy)
            .envs(env.iter().copied())
            .arg("gh")
            .args(args)
            .output()?;
        assert_eq!(output.status.code(), Some(23), "{args:?}: {output:?}");
    }
    Ok(())
}

#[test]
fn every_remote_counts_when_the_command_is_in_doubt() -> TestResult {
    // gh runs `release create`, which honors the saved default, the Forgejo `origin`. The parse
    // sees `auth release`, which ranks `upstream` first. Any Forgejo remote is in reach (Sol r4#1).
    let fixture = CliFixture::new()?;
    let decoy = fixture.write_executable("gh", "#!/bin/sh\necho ran\nexit 23\n")?;
    fixture.git(["init", "-q"])?;
    fixture.git(["remote", "add", "upstream", "https://github.com/o/r.git"])?;
    fixture.git(["remote", "add", "origin", "https://git.example.com/o/r.git"])?;
    fixture.git(["config", "remote.origin.gh-resolved", "base"])?;
    let gh_dir = fixture.home().join(".config/gh");
    std::fs::create_dir_all(&gh_dir)?;
    std::fs::write(
        gh_dir.join("hosts.yml"),
        "github.com:\n    user: u\ngit.example.com:\n    user: u\n",
    )?;
    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_REAL_GH", &decoy)
        .args(["gh", "--notes", "auth", "release", "create", "v1"])
        .output()?;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "plain gh ran");
    Ok(())
}

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
    // No Forgejo host in reach: with one, a call the grammar rejects is refused instead, since a
    // newer gh's flag might take `-R` as its value (`github_target_in_doubt_near_a_forge_is_refused`).
    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_REAL_GH", decoy)
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
