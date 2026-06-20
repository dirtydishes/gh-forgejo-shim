mod support;

use std::collections::BTreeSet;

use support::{load_contract, CliFixture, TestResult};

#[test]
fn long_binary_prints_help_under_isolated_environment() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;

    let output = fixture.command("gh-forgejo-shim")?.arg("--help").output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("gh-forgejo-shim 0.1.1"), "{stdout}");
    assert!(stdout.contains("phase 02 scaffold only"), "{stdout}");
    Ok(())
}

#[test]
fn short_binary_prints_version_without_python() -> TestResult {
    let fixture = CliFixture::new()?;

    let output = fixture.command("gfj")?.arg("--version").output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert_eq!(stdout.trim(), "0.1.1");
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
