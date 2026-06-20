mod support;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;

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
    assert!(stdout.contains("managed gh dispatch is native"), "{stdout}");
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
fn management_commands_remain_explicitly_out_of_scope_for_later_phases() -> TestResult {
    let fixture = CliFixture::new()?;

    let output = fixture.command("gh-forgejo-shim")?.arg("doctor").output()?;

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("not implemented for this surface yet"),
        "{stderr}"
    );
    assert!(
        stderr.contains("managed gh dispatcher is native"),
        "{stderr}"
    );
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
        .arg("gh")
        .arg("pr")
        .arg("view")
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(String::from_utf8(output.stdout)?, "");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("Forgejo route selected for git.example.com"),
        "{stderr}"
    );
    assert!(!stderr.contains("could not find the real gh"), "{stderr}");
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
