mod support;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::process::Command;

use gh_forgejo_shim::VERSION;
use serde_json::Value;
use support::{load_contract, CliFixture, TestResult};

#[test]
fn long_binary_prints_help_under_isolated_environment() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;

    let output = fixture.command("gh-forgejo-shim")?.arg("--help").output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains(&format!("gh-forgejo-shim {VERSION}")),
        "{stdout}"
    );
    assert!(stdout.contains("shim lifecycle"), "{stdout}");
    assert!(stdout.contains("install-shim [--bin-dir DIR] [--force]"));
    assert!(stdout.contains("uninstall-shim [--bin-dir DIR]"));
    assert!(stdout.contains("bootstrap [--bin-dir DIR] [--force]"));
    assert!(stdout.contains("doctor"));
    assert!(
        stdout.contains("config <add-host|remove-host|list>"),
        "{stdout}"
    );
    assert!(
        stdout.contains("auth <login|import|status|logout>"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn short_binary_prints_version_without_python() -> TestResult {
    let fixture = CliFixture::new()?;

    let output = fixture.command("gfj")?.arg("--version").output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(stdout.trim(), VERSION);
    Ok(())
}

#[test]
fn doctor_runs_native_diagnostics() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;

    let output = fixture.command("gh-forgejo-shim")?.arg("doctor").output()?;

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("gh-forgejo-shim doctor"), "{stdout}");
    assert!(stdout.contains("[warn] real gh:"), "{stdout}");
    assert!(stdout.contains("] fj:"), "{stdout}");
    assert!(
        stdout.contains("[ok] forgejo hosts: git.example.com"),
        "{stdout}"
    );
    assert!(stdout.contains("[warn] auth token:"), "{stdout}");
    assert!(stdout.contains("[ok] current repo host:"), "{stdout}");
    assert!(stdout.contains("[warn] shim path:"), "{stdout}");
    Ok(())
}

#[test]
fn bootstrap_installs_shim_allowlists_repo_and_prints_repairs() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let target = fixture.bin().join("gh");

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_TOKEN", "secret-token")
        .args(["bootstrap", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(target.exists());
    let script = fs::read_to_string(&target)?;
    assert!(script.contains("gh-forgejo-shim gh"), "{script}");
    assert!(!script.contains("secret-token"), "{script}");

    let config_path = fixture
        .home()
        .join(".config")
        .join("gh-forgejo-shim")
        .join("config.toml");
    assert_eq!(
        fs::read_to_string(config_path)?,
        "hosts = [\"git.example.com\"]\n"
    );

    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("gh-forgejo-shim bootstrap"), "{stdout}");
    assert!(
        stdout.contains("repo: git.example.com/owner/repo"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "[ok] repository: detected git.example.com/owner/repo and allowlisted git.example.com"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("[ok] shim:"), "{stdout}");
    assert!(stdout.contains("[ok] PATH:"), "{stdout}");
    assert!(
        stdout.contains("[ok] auth: found Forgejo token"),
        "{stdout}"
    );
    assert!(
        stdout.contains("[fix] origin/HEAD: origin/HEAD is not set"),
        "{stdout}"
    );
    assert!(stdout.contains("repair commands:"), "{stdout}");
    assert!(stdout.contains("git remote set-head origin -a"), "{stdout}");
    assert!(!stdout.contains("secret-token"), "{stdout}");
    Ok(())
}

#[test]
fn install_and_uninstall_shim_use_isolated_bin_dir() -> TestResult {
    let fixture = CliFixture::new()?;
    let target = fixture.bin().join("gh");

    let output = fixture
        .command("gfj")?
        .args(["install-shim", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(
        stdout,
        format!("installed gh shim at {}\n", target.display())
    );
    let script = fs::read_to_string(&target)?;
    assert!(script.contains("# managed by gh-forgejo-shim"));
    assert!(script.contains("rollback-compatible marker: gh-forgejo-shim gh"));
    assert!(script.contains(" gh \"$@\""));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        assert_ne!(fs::metadata(&target)?.permissions().mode() & 0o111, 0);
    }

    let output = fixture
        .command("gh-forgejo-shim")?
        .args(["uninstall-shim", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(stdout, format!("removed gh shim at {}\n", target.display()));
    assert!(!target.exists());
    Ok(())
}

#[test]
fn install_shim_refuses_unmanaged_file_until_forced() -> TestResult {
    let fixture = CliFixture::new()?;
    let target = fixture.bin().join("gh");
    fs::write(&target, "unrelated")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .args(["install-shim", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("exists and is not managed by gh-forgejo-shim; pass --force to overwrite"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&target)?, "unrelated");

    let output = fixture
        .command("gh-forgejo-shim")?
        .args(["install-shim", "--force", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert!(output.status.success());
    assert!(fs::read_to_string(&target)?.contains("# managed by gh-forgejo-shim"));
    Ok(())
}

#[test]
fn bootstrap_installs_shim_and_prints_temp_repo_repairs() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let target = fixture.bin().join("gh");

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(target.exists());
    let script = fs::read_to_string(&target)?;
    assert!(script.contains("# managed by gh-forgejo-shim"));
    assert!(!script.contains("bootstrap-token"));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("gh-forgejo-shim bootstrap"), "{stdout}");
    assert!(
        stdout.contains("repo: git.example.com/owner/repo"),
        "{stdout}"
    );
    assert!(stdout.contains("[ok] repository"), "{stdout}");
    assert!(stdout.contains("[ok] shim"), "{stdout}");
    assert!(stdout.contains("[ok] auth"), "{stdout}");
    assert!(stdout.contains("repair commands:"), "{stdout}");
    assert!(!stdout.contains("bootstrap-token"), "{stdout}");
    Ok(())
}

#[test]
fn managed_gh_version_delegates_to_real_gh() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\necho 'gh version 9.9.9 (fake)'\n")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .arg("gh")
        .arg("--version")
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(stdout, "gh version 9.9.9 (fake)\n");
    Ok(())
}

#[test]
fn managed_gh_missing_real_gh_preserves_error_shape() -> TestResult {
    let fixture = CliFixture::new()?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("PATH", "")
        .env("FJ_SHIM_REAL_GH", "/missing/gh")
        .arg("gh")
        .arg("--version")
        .output()?;

    assert_eq!(output.status.code(), Some(127));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("could not find the real gh executable; set FJ_SHIM_REAL_GH"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn managed_gh_forgejo_repo_selects_native_route_without_delegating() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.write_executable("gh", "#!/bin/sh\necho delegated\nexit 17\n")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "secret-token")
        .arg("gh")
        .arg("auth")
        .arg("status")
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("git.example.com"), "{stdout}");
    assert!(stdout.contains("Logged in"), "{stdout}");
    assert!(!stdout.contains("secret-token"), "{stdout}");
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(stderr, "");
    assert!(!stderr.contains("could not find the real gh"), "{stderr}");
    assert!(!stdout.contains("delegated"), "{stdout}");
    Ok(())
}

#[test]
fn managed_gh_github_repo_delegates_even_when_allowlisted() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\necho delegated-github\n")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_HOSTS", "github.com,git.example.com")
        .arg("gh")
        .arg("pr")
        .arg("view")
        .arg("-R")
        .arg("github.com/owner/repo")
        .output()?;

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "delegated-github\n");
    Ok(())
}

#[test]
fn managed_gh_delegation_writes_minimal_redacted_trace() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\necho traced\n")?;
    let trace_path = fixture.root().join("trace.jsonl");

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_TRACE", &trace_path)
        .arg("gh")
        .arg("--version")
        .arg("--token")
        .arg("secret-token")
        .output()?;

    assert!(output.status.success());
    let trace = fs::read_to_string(trace_path)?;
    let record: Value = serde_json::from_str(trace.trim())?;
    assert_eq!(record["kind"], "gh");
    assert_eq!(record["route"]["kind"], "delegate");
    assert_eq!(record["route"]["reason"], "unsupported command");
    assert_eq!(record["argv"][2], "<redacted>");
    assert_eq!(record["stdout"]["bytes"], Value::Null);
    Ok(())
}

#[test]
fn managed_gh_forgejo_auth_status_records_trace() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.write_executable("gh", "#!/bin/sh\necho delegated\nexit 17\n")?;
    let trace_path = fixture.root().join("forgejo-trace.jsonl");

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "secret-token")
        .env("FJ_SHIM_TRACE", &trace_path)
        .arg("gh")
        .arg("auth")
        .arg("status")
        .output()?;

    assert!(output.status.success());
    let trace = fs::read_to_string(trace_path)?;
    let record: Value = serde_json::from_str(trace.trim())?;
    assert_eq!(record["kind"], "gh");
    assert_eq!(record["route"]["kind"], "forgejo");
    assert_eq!(record["route"]["reason"], "git remote");
    assert_eq!(record["host"], "git.example.com");
    assert_eq!(record["repo"]["full_name"], "owner/repo");
    assert_eq!(record["exit_code"], 0);
    assert!(!trace.contains("secret-token"));
    Ok(())
}

#[test]
fn managed_gh_trace_can_use_local_path() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\necho traced\n")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_REAL_GH")
        .env("FJ_SHIM_TRACE", "trace.jsonl")
        .arg("gh")
        .arg("--version")
        .output()?;

    assert!(output.status.success());
    let trace_path = fixture.repo().join("trace.jsonl");
    let trace = fs::read_to_string(trace_path)?;
    let record: Value = serde_json::from_str(trace.trim())?;
    assert_eq!(record["kind"], "gh");
    assert_eq!(record["route"]["kind"], "delegate");
    Ok(())
}

#[test]
fn trace_summarize_reports_routes_probes_and_failures() -> TestResult {
    let fixture = CliFixture::new()?;
    let trace_path = fixture.root().join("trace.jsonl");
    fs::write(
        &trace_path,
        r#"{"kind":"gh","argv":["pr","status"],"exit_code":1,"duration_ms":25,"route":{"kind":"forgejo","reason":"git remote"}}"#,
    )?;
    fs::write(
        &trace_path,
        format!(
            "{}\n{}\n",
            fs::read_to_string(&trace_path)?,
            r#"{"kind":"git","argv":["branch","--show-current"],"exit_code":0,"duration_ms":5}"#
        ),
    )?;

    let output = fixture
        .command("gfj")?
        .args(["trace", "summarize"])
        .arg(&trace_path)
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("Trace records: 2"), "{stdout}");
    assert!(stdout.contains("Routes: forgejo=1"), "{stdout}");
    assert!(stdout.contains("Known Codex probes:"), "{stdout}");
    assert!(stdout.contains("gh pr status=1"), "{stdout}");
    assert!(stdout.contains("git current branch=1"), "{stdout}");
    assert!(stdout.contains("Failures: 1"), "{stdout}");
    Ok(())
}

#[test]
fn trace_smoke_uses_fake_gh_and_git_in_temp_path() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.write_executable("gh", "#!/bin/sh\necho fake-gh\n")?;
    fixture.write_executable("git", "#!/bin/sh\necho fake-git\n")?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .args(["trace", "smoke", "--cwd"])
        .arg(fixture.repo())
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("Codex probe smoke results:"), "{stdout}");
    assert!(stdout.contains("[ok] gh version"), "{stdout}");
    assert!(stdout.contains("[ok] git repo root"), "{stdout}");
    assert!(stdout.contains("Required probes passed."), "{stdout}");
    Ok(())
}

#[test]
fn trace_git_recorder_captures_fake_git_and_removes_wrapper() -> TestResult {
    let fixture = CliFixture::new()?;
    let real_git = fixture.write_executable(
        "real-git",
        "#!/bin/sh\necho real-git \"$@\"\necho stderr-$1 >&2\n",
    )?;
    let trace_path = fixture.root().join("git-trace.jsonl");

    let output = fixture
        .command("gfj")?
        .args(["trace", "git-recorder", "create"])
        .arg(&trace_path)
        .args(["--real-git"])
        .arg(&real_git)
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    let wrapper_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("created temporary git recorder at "))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, stdout.clone()))?;
    let wrapper_path = std::path::PathBuf::from(wrapper_path);
    assert!(wrapper_path.exists());

    let output = Command::new(&wrapper_path)
        .arg("status")
        .arg("--token")
        .arg("secret-value")
        .output()?;

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "real-git status --token secret-value\n"
    );
    let record: Value = serde_json::from_str(fs::read_to_string(&trace_path)?.trim())?;
    assert_eq!(record["kind"], "git");
    assert_eq!(record["argv"][0], "status");
    assert_eq!(record["argv"][1], "--token");
    assert_eq!(record["argv"][2], "[REDACTED]");
    assert_eq!(record["stdout"]["bytes"], 37);
    assert!(!fs::read_to_string(&trace_path)?.contains("secret-value"));

    let output = fixture
        .command("gfj")?
        .args(["trace", "git-recorder", "remove"])
        .arg(wrapper_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "wrapper path has no parent")
        })?)
        .output()?;

    assert!(output.status.success());
    assert!(!wrapper_path.exists());
    Ok(())
}

#[test]
fn config_add_host_persists_python_readable_config() -> TestResult {
    let fixture = CliFixture::new()?;

    let mut add = fixture.command("gh-forgejo-shim")?;
    add.env_remove("FJ_SHIM_HOSTS")
        .env_remove("FJ_SHIM_REAL_GH")
        .args(["config", "add-host", "https://Git.Example.com/path"]);
    let output = add.output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("added Forgejo host: https://Git.Example.com/path"));
    assert!(stdout.contains("git.example.com"));

    let config_path = fixture
        .home()
        .join(".config")
        .join("gh-forgejo-shim")
        .join("config.toml");
    let text = fs::read_to_string(config_path)?;
    assert_eq!(text, "hosts = [\"git.example.com\"]\n");

    let mut list = fixture.command("gfj")?;
    list.env_remove("FJ_SHIM_HOSTS")
        .env_remove("FJ_SHIM_REAL_GH")
        .args(["config", "list"]);
    let output = list.output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "git.example.com\n");
    Ok(())
}

#[test]
fn auth_status_reports_stored_auth_without_secret() -> TestResult {
    let fixture = CliFixture::new()?;
    let host = format!("auth-test-{}.invalid", std::process::id());
    let config_dir = fixture.home().join(".config").join("gh-forgejo-shim");
    fs::create_dir_all(&config_dir)?;
    fs::write(
        config_dir.join("config.toml"),
        format!("hosts = [\"{host}\"]\n"),
    )?;
    fs::write(
        config_dir.join("auth.json"),
        format!("{{\"hosts\":{{\"{host}\":{{\"token\":\"stored-secret\"}}}}}}\n"),
    )?;

    let mut command = fixture.command("gfj")?;
    command.env_remove("FJ_SHIM_HOSTS").args(["auth", "status"]);
    let output = command.output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains(&format!("{host}: logged in")), "{stdout}");
    assert!(!stdout.contains("stored-secret"), "{stdout}");
    Ok(())
}

#[test]
fn fixture_exposes_isolated_paths_and_fake_bin_precedes_system_path() -> TestResult {
    let fixture = CliFixture::new()?;

    assert!(fixture.root().is_dir(), "{:?}", fixture.root());
    assert!(fixture.home().is_dir(), "{:?}", fixture.home());
    assert!(fixture.repo().is_dir(), "{:?}", fixture.repo());
    assert!(fixture.bin().is_dir(), "{:?}", fixture.bin());

    let fake = fixture.write_executable("gh", "#!/bin/sh\nexit 0\n")?;
    assert!(fake.exists(), "{fake:?}");

    let command = fixture.command("gh-forgejo-shim")?;
    let path_value = command
        .get_envs()
        .find_map(|(name, value)| (name == OsStr::new("PATH")).then_some(value))
        .flatten()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "fixture PATH is not set"))?;
    let first_path = std::env::split_paths(path_value)
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "fixture PATH is empty"))?;

    assert_eq!(first_path, fixture.bin());
    Ok(())
}

#[test]
fn harness_loads_phase_01_compatibility_contract() -> TestResult {
    let contract = load_contract()?;

    assert_eq!(contract.schema, "gh-forgejo-shim.compatibility-contract.v1");
    assert_eq!(contract.bead, "gh-forgejo-shim-3sl");

    let command_ids = contract
        .command_matrix
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<BTreeSet<_>>();
    assert!(command_ids.contains("management.version"));
    assert!(command_ids.contains("managed_gh.pr_list"));
    assert!(command_ids.contains("managed_gh.unsupported_or_github"));

    let golden_probe_ids = contract
        .golden_probes
        .iter()
        .map(|probe| probe.id.as_str())
        .collect::<BTreeSet<_>>();
    assert!(golden_probe_ids.contains("gh_version_delegated"));
    assert!(golden_probe_ids.contains("gh_pr_checks_json"));
    assert!(golden_probe_ids.contains("git_upstream_trace"));
    Ok(())
}
