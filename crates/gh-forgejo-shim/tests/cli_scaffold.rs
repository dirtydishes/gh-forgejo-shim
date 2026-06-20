mod support;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io;

use gh_forgejo_shim::VERSION;
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
    assert!(stdout.contains("phase 02 scaffold only"), "{stdout}");
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
fn behavior_commands_are_explicitly_out_of_scope_for_phase_02() -> TestResult {
    let fixture = CliFixture::new()?;

    let output = fixture.command("gh-forgejo-shim")?.arg("doctor").output()?;

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("Rust scaffold only"), "{stderr}");
    assert!(stderr.contains("phase 02"), "{stderr}");
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
