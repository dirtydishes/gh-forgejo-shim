#[path = "support/config_credentials.rs"]
mod config_credentials;
#[path = "support/invocation.rs"]
mod invocation;
mod support;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::process::Command;

use gh_forgejo_shim::VERSION;
use serde_json::Value;
use support::{load_contract, write_script_from_child, CliFixture, TestResult};

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
    assert!(
        stdout.contains(
            "bootstrap [--target current|user-local] [--bin-dir DIR] [--force] [--no-gui-path] [--dry-run]"
        ),
        "{stdout}"
    );
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

/// The shim's config text, or "" when bootstrap wrote none. Any other read error fails the test.
fn shim_config(fixture: &CliFixture) -> io::Result<String> {
    match fs::read_to_string(fixture.home().join(".config/gh-forgejo-shim/config.toml")) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        result => result,
    }
}

/// Lists `hosts` in the user's own gh `hosts.yml`, as `gh auth login` would.
fn write_gh_hosts(fixture: &CliFixture, hosts: &[&str]) -> io::Result<()> {
    let dir = fixture.home().join(".config/gh");
    fs::create_dir_all(&dir)?;
    let text = hosts
        .iter()
        .map(|host| format!("{host}:\n    user: me\n"))
        .collect::<String>();
    fs::write(dir.join("hosts.yml"), text)
}

fn bootstrap(fixture: &CliFixture, env: &[(&str, &str)]) -> io::Result<String> {
    let output = fixture
        .command("gfj")?
        .env_remove("FJ_SHIM_HOSTS")
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .envs(env.iter().copied())
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn doctor(fixture: &CliFixture, env: &[(&str, &str)]) -> io::Result<String> {
    let mut command = fixture.command("gh-forgejo-shim")?;
    command.envs(env.iter().copied()).arg("doctor");
    Ok(String::from_utf8_lossy(&command.output()?.stdout).into_owned())
}

/// A fork: origin on GitHub, upstream on a Forgejo host. gh ranks `upstream` above `origin`.
fn fork_fixture(forge: &str) -> Result<CliFixture, Box<dyn std::error::Error>> {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.git([
        "remote",
        "set-url",
        "origin",
        "https://github.com/someone/repo.git",
    ])?;
    fixture.git([
        "remote",
        "add",
        "upstream",
        &format!("https://{forge}/owner/repo.git"),
    ])?;
    Ok(fixture)
}

#[test]
fn doctor_names_the_repository_the_wrapper_routes_in_a_fork() -> TestResult {
    // The wrapper routes the allowlisted upstream; the first remote, origin, is GitHub.
    let fixture = fork_fixture("git.example.com")?;

    let output = fixture.command("gh-forgejo-shim")?.arg("doctor").output()?;

    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("[ok] current repo host: git.example.com is allowlisted for owner/repo"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn doctor_matches_a_remote_against_an_allowlisted_host_with_its_port() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.git([
        "remote",
        "set-url",
        "origin",
        "https://git.example.com:3000/owner/repo.git",
    ])?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_HOSTS", "git.example.com:3000")
        .arg("doctor")
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("[ok] current repo host: git.example.com"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn bootstrap_first_run_allowlists_a_forge_gh_does_not_know_yet() -> TestResult {
    // gh skips a remote on a host it doesn't know, so here it resolves nothing; bootstrap still
    // finds origin and allowlists its host.
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .env_remove("FJ_SHIM_HOSTS")
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("detected git.example.com/owner/repo and allowlisted git.example.com"),
        "{stdout}"
    );
    let config_path = fixture.home().join(".config/gh-forgejo-shim/config.toml");
    assert_eq!(
        fs::read_to_string(config_path)?,
        "hosts = [\"git.example.com\"]\n"
    );
    Ok(())
}

#[test]
fn bootstrap_never_allowlists_a_github_enterprise_host_from_the_users_gh_hosts() -> TestResult {
    // gh resolves the upstream through the user's own hosts.yml, so it is real gh's, though its
    // name isn't github.com. Behind a GitHub origin, or a forge origin gh doesn't know.
    for origin in [
        "https://github.com/me/proj.git",
        "https://forge.example.com/me/proj.git",
    ] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        write_gh_hosts(&fixture, &["github.com", "ghe.corp.example"])?;
        fixture.git(["remote", "set-url", "origin", origin])?;
        fixture.git([
            "remote",
            "add",
            "upstream",
            "https://ghe.corp.example/team/proj.git",
        ])?;

        let stdout = bootstrap(&fixture, &[])?;

        assert!(
            stdout.contains("leaving ghe.corp.example out of the Forgejo allowlist"),
            "{origin}: {stdout}"
        );
        assert_eq!(
            shim_config(&fixture)?,
            "",
            "{origin}: something was allowlisted"
        );
        let stdout = doctor(&fixture, &[("FJ_SHIM_HOSTS", "")])?;
        assert!(
            stdout.contains(
                "[ok] current repo host: ghe.corp.example will delegate to the real GitHub CLI"
            ),
            "{origin}: {stdout}"
        );
    }
    Ok(())
}

/// A forge origin with a GitHub upstream, which gh ranks first among remotes it knows.
fn forge_origin_github_upstream() -> Result<CliFixture, Box<dyn std::error::Error>> {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.git([
        "remote",
        "set-url",
        "origin",
        "https://forge.example.com/me/proj.git",
    ])?;
    fixture.git([
        "remote",
        "add",
        "upstream",
        "https://github.com/orig/proj.git",
    ])?;
    Ok(fixture)
}

#[test]
fn bootstrap_allowlists_a_forge_origin_the_wrapper_would_then_route() -> TestResult {
    // With no gh hosts.yml, the allowlisted forge becomes gh's only configured host, so gh
    // prefers its remote and the wrapper routes this checkout there.
    let fixture = forge_origin_github_upstream()?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(
        stdout.contains("detected forge.example.com/me/proj and allowlisted forge.example.com"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, "hosts = [\"forge.example.com\"]\n");
    Ok(())
}

#[test]
fn bootstrap_leaves_out_a_forge_origin_gh_would_still_pass_over() -> TestResult {
    // A hosts.yml naming github.com keeps it gh's default, so the GitHub upstream wins whatever
    // the allowlist says; bootstrap allowlists nothing and gives the command for other checkouts.
    let fixture = forge_origin_github_upstream()?;
    write_gh_hosts(&fixture, &["github.com"])?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(
        stdout.contains("forge.example.com was left out too"),
        "{stdout}"
    );
    assert!(
        stdout.contains("run gh-forgejo-shim config add-host forge.example.com"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, "", "something was allowlisted");
    Ok(())
}

#[test]
fn bootstrap_never_suggests_allowlisting_an_already_allowlisted_forge() -> TestResult {
    let fixture = forge_origin_github_upstream()?;
    write_gh_hosts(&fixture, &["github.com"])?;
    let config_dir = fixture.home().join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config_dir)?;
    fs::write(
        config_dir.join("config.toml"),
        "hosts = [\"forge.example.com\"]\n",
    )?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(stdout.contains("detected GitHub repository"), "{stdout}");
    assert!(!stdout.contains("add-host"), "{stdout}");
    assert_eq!(shim_config(&fixture)?, "hosts = [\"forge.example.com\"]\n");
    Ok(())
}

#[test]
fn bootstrap_never_suggests_allowlisting_a_github_enterprise_origin() -> TestResult {
    // The origin is a GitHub Enterprise host from the user's hosts.yml; pasting an add-host for
    // it would send the Forgejo token there.
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    write_gh_hosts(&fixture, &["github.com", "ghe.corp.example"])?;
    fixture.git([
        "remote",
        "set-url",
        "origin",
        "https://ghe.corp.example/team/proj.git",
    ])?;
    fixture.git([
        "remote",
        "add",
        "upstream",
        "https://github.com/orig/proj.git",
    ])?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(!stdout.contains("add-host ghe.corp.example"), "{stdout}");
    assert!(
        stdout.contains("ghe.corp.example is in your gh hosts.yml"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, "", "something was allowlisted");
    Ok(())
}

#[test]
fn bootstrap_leaves_out_a_host_the_users_gh_hosts_list_and_says_so() -> TestResult {
    // A Forgejo server can be in hosts.yml too, say after `gh config set -h <host> git_protocol
    // ssh`, and nothing tells it from GitHub Enterprise; bootstrap leaves it out and says so. A
    // key with a port lists the same host, as gh compares hosts.
    for (listed, remote) in [
        ("forge.example.com", "https://forge.example.com/me/proj.git"),
        (
            "ghe.corp.example:8443",
            "https://ghe.corp.example:8443/team/proj.git",
        ),
    ] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        write_gh_hosts(&fixture, &[listed])?;
        fixture.git(["remote", "set-url", "origin", remote])?;

        let stdout = bootstrap(&fixture, &[])?;

        assert!(
            stdout.contains("is in your gh hosts.yml"),
            "{listed}: {stdout}"
        );
        assert!(
            stdout.contains("allowlist it with gh-forgejo-shim config add-host HOST"),
            "{listed}: {stdout}"
        );
        assert_eq!(shim_config(&fixture)?, "", "{listed} was allowlisted");
    }
    Ok(())
}

#[test]
fn bootstrap_allowlists_a_forge_that_gh_host_or_gh_repo_picks() -> TestResult {
    // On a first run, GH_HOST or GH_REPO makes gh resolve the forge; it isn't a host the user
    // logged gh in to, so it's a candidate, and it routes once allowlisted.
    for env in [
        ("GH_HOST", "forge.example.com"),
        ("GH_REPO", "forge.example.com/me/proj"),
    ] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        fixture.git([
            "remote",
            "set-url",
            "origin",
            "https://forge.example.com/me/proj.git",
        ])?;

        let stdout = bootstrap(&fixture, &[env])?;

        assert!(
            stdout.contains("detected forge.example.com/me/proj and allowlisted forge.example.com"),
            "{env:?}: {stdout}"
        );
        assert_eq!(
            shim_config(&fixture)?,
            "hosts = [\"forge.example.com\"]\n",
            "{env:?}"
        );
    }
    Ok(())
}

#[test]
fn doctor_follows_plain_gh_to_a_forge_its_own_hosts_name() -> TestResult {
    // With two allowlisted hosts, the launched gh ranks the GitHub upstream first; plain gh,
    // whose hosts.yml names only the forge, picks origin, and the wrapper routes that.
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    write_gh_hosts(&fixture, &["git.example.com"])?;
    fixture.git([
        "remote",
        "add",
        "upstream",
        "https://github.com/orig/repo.git",
    ])?;

    let stdout = doctor(
        &fixture,
        &[("FJ_SHIM_HOSTS", "git.example.com,other.example.com")],
    )?;

    assert!(
        stdout.contains("[ok] current repo host: git.example.com is allowlisted for owner/repo"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn doctor_names_no_repository_where_gh_host_finds_none() -> TestResult {
    // GH_HOST=github.com keeps only GitHub remotes; there are none, so the wrapper resolves no
    // repository and neither should doctor.
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;

    // A GH_HOST of spaces pins gh to that host just the same.
    for env in [
        ("GH_HOST", "github.com"),
        ("GH_HOST", " "),
        ("GH_REPO", "not a repository"),
    ] {
        let stdout = doctor(&fixture, &[env])?;
        assert!(!stdout.contains("current repo host"), "{env:?}: {stdout}");
    }
    Ok(())
}

#[test]
fn bootstrap_keeps_the_port_of_an_allowlisted_host() -> TestResult {
    // gh drops the remote's port, so gh's spelling is git.example.com; adding that beside
    // git.example.com:3000 would leave two entries for one hostname, and neither would route.
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.git([
        "remote",
        "set-url",
        "origin",
        "https://git.example.com:3000/owner/repo.git",
    ])?;
    let config_dir = fixture.home().join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config_dir)?;
    fs::write(
        config_dir.join("config.toml"),
        "hosts = [\"git.example.com:3000\"]\n",
    )?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(
        stdout.contains("allowlisted git.example.com:3000"),
        "{stdout}"
    );
    assert_eq!(
        shim_config(&fixture)?,
        "hosts = [\"git.example.com:3000\"]\n"
    );
    Ok(())
}

/// A checkout whose only remote is origin at `url`, with `ssh -G` answering `ssh_host` for any
/// alias, as the user's SSH config would.
fn checkout_with_origin(
    url: &str,
    ssh_host: &str,
) -> Result<CliFixture, Box<dyn std::error::Error>> {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    fixture.git(["remote", "set-url", "origin", url])?;
    fixture.write_executable("ssh", &format!("#!/bin/sh\necho 'hostname {ssh_host}'\n"))?;
    Ok(fixture)
}

#[test]
fn bootstrap_never_allowlists_a_remote_gh_reads_differently() -> TestResult {
    // gh drops `www.`, resolves an SSH alias with `ssh -G`, and can't parse a path with three
    // parts, so it resolves no repository here even with the remote's host allowlisted, and the
    // wrapper delegates. Allowlisting the host as git spells it would route nothing.
    for (url, ssh_host) in [
        ("https://www.forge.example.com/me/proj.git", "unused"),
        ("git@forge-alias:me/proj.git", "forge.example.com"),
        ("https://forge.example.com/forgejo/me/proj.git", "unused"),
    ] {
        let fixture = checkout_with_origin(url, ssh_host)?;

        let stdout = bootstrap(&fixture, &[])?;

        assert!(!stdout.contains("allowlisted"), "{url}: {stdout}");
        assert!(
            !stdout.contains("config add-host www")
                && !stdout.contains("config add-host forge-alias"),
            "{url}: a command names a host gh never uses: {stdout}"
        );
        assert_eq!(shim_config(&fixture)?, "", "{url} was allowlisted");
    }
    Ok(())
}

#[test]
fn bootstrap_never_names_an_ssh_alias_of_github_in_a_command() -> TestResult {
    // The usual setup for two GitHub accounts: gh turns github-work into github.com everywhere.
    let fixture = checkout_with_origin("git@github-work:org/repo.git", "github.com")?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(!stdout.contains("add-host github-work"), "{stdout}");
    assert!(
        stdout.contains("detected GitHub repository github.com/org/repo"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, "", "something was allowlisted");
    Ok(())
}

#[test]
fn bootstrap_reports_two_entries_for_one_hostname_and_adds_none() -> TestResult {
    // Two entries for one hostname route neither; a third, or a hint to add one, can't help.
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;
    let config_dir = fixture.home().join(".config/gh-forgejo-shim");
    fs::create_dir_all(&config_dir)?;
    let config = "hosts = [\"forge.example.com\", \"forge.example.com:3000\"]\n";
    fs::write(config_dir.join("config.toml"), config)?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(
        stdout.contains("the allowlist has more than one entry for forge.example.com"),
        "{stdout}"
    );
    assert!(!stdout.contains("config add-host"), "{stdout}");
    assert_eq!(shim_config(&fixture)?, config);
    Ok(())
}

#[test]
fn bootstrap_says_when_fj_shim_hosts_hides_the_entry_it_wrote() -> TestResult {
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;

    let stdout = bootstrap(&fixture, &[("FJ_SHIM_HOSTS", "other.example.com")])?;

    assert!(stdout.contains("allowlisted forge.example.com"), "{stdout}");
    assert!(
        stdout.contains("FJ_SHIM_HOSTS is set, and replaces the config file's hosts"),
        "{stdout}"
    );
    Ok(())
}

fn write_shim_config(fixture: &CliFixture, text: &str) -> io::Result<()> {
    let dir = fixture.home().join(".config/gh-forgejo-shim");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("config.toml"), text)
}

#[test]
fn bootstrap_never_names_a_github_enterprise_host_in_auth_advice() -> TestResult {
    // gh resolves the GHES upstream; the forge origin's two entries conflict. Auth advice that
    // named the GHES host would send it the Forgejo token when the user ran it.
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;
    write_gh_hosts(&fixture, &["github.com", "ghe.corp.example"])?;
    fixture.git([
        "remote",
        "add",
        "upstream",
        "https://ghe.corp.example/team/proj.git",
    ])?;
    write_shim_config(
        &fixture,
        "hosts = [\"forge.example.com\", \"forge.example.com:3000\"]\n",
    )?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(!stdout.contains("auth login ghe.corp.example"), "{stdout}");
    assert!(!stdout.contains("add-host ghe.corp.example"), "{stdout}");
    Ok(())
}

#[test]
fn doctor_never_names_a_github_enterprise_host_from_hosts_yml_in_a_command() -> TestResult {
    // The hosts.yml key carries a port, so gh resolves nothing and doctor falls back to origin.
    let fixture = checkout_with_origin("https://ghe.corp.example:8443/team/proj.git", "unused")?;
    write_gh_hosts(&fixture, &["ghe.corp.example:8443"])?;

    let stdout = doctor(&fixture, &[("FJ_SHIM_HOSTS", "")])?;

    assert!(!stdout.contains("add-host ghe.corp.example"), "{stdout}");
    assert!(
        stdout.contains("ghe.corp.example is in your gh hosts.yml"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn bootstrap_reports_one_entry_with_another_port_as_a_mismatch() -> TestResult {
    // One entry, not two: it doesn't serve the :8443 repository, and a second entry for the
    // hostname would leave both unrouted.
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;
    let config = "hosts = [\"forge.example.com:3000\"]\n";
    write_shim_config(&fixture, config)?;

    let stdout = bootstrap(
        &fixture,
        &[("GH_REPO", "forge.example.com:8443/owner/repo")],
    )?;

    assert!(!stdout.contains("more than one entry"), "{stdout}");
    assert!(
        stdout.contains("forge.example.com:3000 doesn't serve forge.example.com:8443"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, config);
    Ok(())
}

#[test]
fn bootstrap_says_why_gh_acts_on_no_remote_when_gh_host_points_elsewhere() -> TestResult {
    // GH_HOST=github.com keeps only GitHub remotes, of which there are none, so the wrapper
    // hands this checkout to real gh; the forge origin can still be allowlisted elsewhere.
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;

    let stdout = bootstrap(&fixture, &[("GH_HOST", "github.com")])?;

    assert!(
        stdout.contains("gh resolves no repository here (GH_HOST is github.com)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("run gh-forgejo-shim config add-host forge.example.com"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("could not detect a Forgejo repository"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, "", "something was allowlisted");
    Ok(())
}

#[test]
fn bootstrap_gives_no_auth_advice_for_an_allowlisted_github_enterprise_host() -> TestResult {
    // The user allowlisted a host their gh hosts.yml lists. Bootstrap writes nothing for it and
    // never offers `auth login` or `auth import` for it: either would send it the Forgejo token.
    let fixture = checkout_with_origin("https://ghe.corp.example/me/proj.git", "unused")?;
    write_gh_hosts(&fixture, &["ghe.corp.example"])?;

    let stdout = bootstrap(&fixture, &[("FJ_SHIM_HOSTS", "ghe.corp.example")])?;

    assert!(!stdout.contains("auth login ghe.corp.example"), "{stdout}");
    assert!(!stdout.contains("--hostname ghe.corp.example"), "{stdout}");
    assert!(
        stdout.contains("ghe.corp.example is in your gh hosts.yml and in the Forgejo allowlist"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, "", "bootstrap wrote the GHES host");
    Ok(())
}

#[test]
fn bootstrap_suggests_removing_the_exact_entry_that_routes_github_enterprise() -> TestResult {
    // gh drops the port, but the entry keeps it; `remove-host` with gh's spelling would succeed
    // and leave the route in place.
    let fixture = checkout_with_origin("https://ghe.corp.example:8443/me/proj.git", "unused")?;
    write_gh_hosts(&fixture, &["ghe.corp.example"])?;
    write_shim_config(&fixture, "hosts = [\"ghe.corp.example:8443\"]\n")?;

    let stdout = bootstrap(&fixture, &[])?;

    assert!(
        stdout.contains("run gh-forgejo-shim config remove-host ghe.corp.example:8443"),
        "{stdout}"
    );
    let removed = fixture
        .command("gh-forgejo-shim")?
        .env_remove("FJ_SHIM_HOSTS")
        .args(["config", "remove-host", "ghe.corp.example:8443"])
        .output()?;
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(
        shim_config(&fixture)?,
        "hosts = []\n",
        "the entry is still there"
    );

    // From FJ_SHIM_HOSTS, no config command can remove it.
    let fixture = checkout_with_origin("https://ghe.corp.example/me/proj.git", "unused")?;
    write_gh_hosts(&fixture, &["ghe.corp.example"])?;
    let stdout = bootstrap(&fixture, &[("FJ_SHIM_HOSTS", "ghe.corp.example")])?;
    assert!(
        stdout.contains("remove ghe.corp.example from FJ_SHIM_HOSTS"),
        "{stdout}"
    );
    assert!(!stdout.contains("config remove-host"), "{stdout}");
    Ok(())
}

#[test]
fn bootstrap_quotes_an_ipv6_entry_in_its_repair_command() -> TestResult {
    // `[::1]:8443` is a glob pattern to the shell: unquoted, a file named `1:8443` in the
    // current directory would replace it, and remove-host would remove nothing.
    let fixture = checkout_with_origin("https://[::1]:8443/me/proj.git", "unused")?;
    write_gh_hosts(&fixture, &["::1"])?;
    write_shim_config(&fixture, "hosts = [\"[::1]:8443\"]\n")?;
    fs::write(fixture.repo().join("1:8443"), "")?;

    let stdout = bootstrap(&fixture, &[])?;

    let quoted = "'[::1]:8443'";
    assert!(
        stdout.contains(&format!("run gh-forgejo-shim config remove-host {quoted}")),
        "{stdout}"
    );
    let echoed = Command::new("/bin/sh")
        .args(["-c", &format!("printf '%s' {quoted}")])
        .current_dir(fixture.repo())
        .output()?;
    assert_eq!(String::from_utf8(echoed.stdout)?, "[::1]:8443");
    Ok(())
}

#[test]
fn bootstrap_reads_the_remote_spelling_even_with_gh_repo_set() -> TestResult {
    // GH_REPO wins over the remotes; the spelling check must look past it.
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;

    let stdout = bootstrap(&fixture, &[("GH_REPO", "not a repository")])?;

    assert!(
        stdout.contains("run gh-forgejo-shim config add-host forge.example.com"),
        "{stdout}"
    );
    assert!(!stdout.contains("isn't a host gh reads"), "{stdout}");
    Ok(())
}

#[test]
fn bootstrap_never_says_it_leaves_out_a_host_already_allowlisted() -> TestResult {
    let fixture = checkout_with_origin("https://forge.example.com/me/proj.git", "unused")?;
    let config = "hosts = [\"forge.example.com\"]\n";
    write_shim_config(&fixture, config)?;

    let stdout = bootstrap(&fixture, &[("GH_HOST", "github.com")])?;

    assert!(
        stdout.contains("forge.example.com is already in the Forgejo allowlist"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("leaving forge.example.com out"),
        "{stdout}"
    );
    assert_eq!(shim_config(&fixture)?, config);
    Ok(())
}

#[test]
fn bootstrap_in_a_fork_leaves_an_unknown_forge_upstream_alone() -> TestResult {
    // Unchanged from before: gh knows github.com and not the forge, so it resolves the GitHub
    // origin, and bootstrap allowlists nothing. Run `config add-host` for the forge instead.
    let fixture = fork_fixture("forge.example.com")?;

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .env_remove("FJ_SHIM_HOSTS")
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("detected GitHub repository github.com/someone/repo"),
        "{stdout}"
    );
    assert!(
        !shim_config(&fixture)?.contains("forge"),
        "the forge was allowlisted"
    );
    Ok(())
}

#[test]
fn doctor_ignores_an_installed_system_gh() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;

    let output = fixture.command("gh-forgejo-shim")?.arg("doctor").output()?;

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("gh-forgejo-shim doctor"), "{stdout}");
    assert!(stdout.contains("[fix] current PATH:"), "{stdout}");
    assert!(stdout.contains("[fix] real gh:"), "{stdout}");
    assert!(stdout.contains("[fix] managed gh:"), "{stdout}");
    assert!(stdout.contains("] bd:"), "{stdout}");
    assert!(stdout.contains("] fj:"), "{stdout}");
    assert!(
        stdout.contains("[ok] forgejo hosts: git.example.com"),
        "{stdout}"
    );
    assert!(stdout.contains("[fix] auth token:"), "{stdout}");
    assert!(stdout.contains("[ok] current repo host:"), "{stdout}");
    assert!(stdout.contains("repair commands:"), "{stdout}");
    assert!(!stdout.contains("/usr/bin/gh"), "{stdout}");
    Ok(())
}

#[test]
fn bootstrap_installs_shim_allowlists_repo_and_prints_repairs() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let target = fixture.bin().join("gh");

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .env("FJ_SHIM_TOKEN", "secret-token")
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
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
    assert!(stdout.contains("[ok] current PATH:"), "{stdout}");
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
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
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
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
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
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .args(["install-shim", "--force", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert!(output.status.success());
    assert!(fs::read_to_string(&target)?.contains("# managed by gh-forgejo-shim"));
    Ok(())
}

#[test]
fn bootstrap_leaves_out_a_remote_host_spelled_differently_from_where_it_leads() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    // `%2e` is a dot to the URL parser, so this remote leads to forge.example.com.
    fixture.git([
        "remote",
        "set-url",
        "origin",
        "https://forge.example%2ecom/owner/repo.git",
    ])?;

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    let stdout = String::from_utf8(output.stdout)?;
    assert!(!stdout.contains("allowlisted"), "{stdout}");
    assert!(
        stdout.contains("isn't a host name the shim can use"),
        "{stdout}"
    );
    let config_path = fixture.home().join(".config/gh-forgejo-shim/config.toml");
    assert!(
        !fs::read_to_string(config_path)
            .unwrap_or_default()
            .contains("forge"),
        "the spelling was allowlisted"
    );
    Ok(())
}

#[test]
fn bootstrap_installs_shim_and_prints_temp_repo_repairs() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let target = fixture.bin().join("gh");

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", fixture.supported_gh()?)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
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
fn bootstrap_default_current_target_uses_visible_user_dir() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let cargo_bin = fixture.home().join(".cargo").join("bin");
    fs::create_dir_all(&cargo_bin)?;
    let real_gh = fixture.supported_gh()?;

    let output = fixture
        .command("gfj")?
        .env(
            "PATH",
            std::env::join_paths([cargo_bin.as_path(), fixture.bin()])?,
        )
        .env("FJ_SHIM_REAL_GH", &real_gh)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--no-gui-path"])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(cargo_bin.join("gh").exists());
    assert!(!fixture
        .home()
        .join(".local")
        .join("bin")
        .join("gh")
        .exists());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("[ok] real gh:"), "{stdout}");
    assert!(stdout.contains("[ok] current PATH:"), "{stdout}");
    assert!(
        stdout.contains(cargo_bin.to_string_lossy().as_ref()),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn bootstrap_default_current_target_falls_back_to_user_local_without_safe_visible_dir() -> TestResult
{
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let real_gh = fixture.supported_gh()?;
    let target = fixture.home().join(".local").join("bin").join("gh");

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &real_gh)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--no-gui-path"])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(target.exists());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("[fix] current PATH:"), "{stdout}");
    assert!(stdout.contains("is not in current PATH"), "{stdout}");
    assert!(stdout.contains("export PATH="), "{stdout}");
    Ok(())
}

#[test]
fn bootstrap_dry_run_writes_nothing_and_prints_planned_actions() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let real_gh = fixture.supported_gh()?;
    let target = fixture.bin().join("gh");
    let config_path = fixture
        .home()
        .join(".config")
        .join("gh-forgejo-shim")
        .join("config.toml");

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &real_gh)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--dry-run", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(!target.exists());
    assert!(!config_path.exists());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("[plan] repository:"), "{stdout}");
    assert!(stdout.contains("[plan] shim:"), "{stdout}");
    assert!(
        stdout.contains("dry run: no files were written"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn bootstrap_refuses_unmanaged_target_until_forced() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let real_gh = fixture.supported_gh()?;
    let target = fixture.bin().join("gh");
    fs::write(&target, "unrelated")?;

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &real_gh)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_to_string(&target)?, "unrelated");
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("[fix] shim:"), "{stdout}");
    assert!(
        stdout.contains("is not managed by gh-forgejo-shim"),
        "{stdout}"
    );

    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &real_gh)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .args(["bootstrap", "--force", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(fs::read_to_string(&target)?.contains("# managed by gh-forgejo-shim"));
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
fn managed_gh_forgejo_auth_status_launches_without_delegating() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    // Real gh: a launched `auth status` runs it with the run directory as its config (L3);
    // a delegation would leave GH_CONFIG_DIR alone.
    let real_gh = fixture.write_executable(
        "gh",
        "#!/bin/sh\ncase \"$GH_CONFIG_DIR\" in */gfj-*) echo \"launched for git.example.com: Logged in\" ;; *) echo delegated; exit 17 ;; esac\n",
    )?;

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "secret-token")
        .env("FJ_SHIM_REAL_GH", &real_gh)
        .arg("gh")
        .arg("auth")
        .arg("status")
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("launched for git.example.com"), "{stdout}");
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
    // Real gh: a launched `auth status` runs it with the run directory as its config (L3);
    // a delegation would leave GH_CONFIG_DIR alone.
    let real_gh = fixture.write_executable(
        "gh",
        "#!/bin/sh\ncase \"$GH_CONFIG_DIR\" in */gfj-*) echo \"launched for git.example.com: Logged in\" ;; *) echo delegated; exit 17 ;; esac\n",
    )?;
    let trace_path = fixture.root().join("forgejo-trace.jsonl");

    let output = fixture
        .command("gh-forgejo-shim")?
        .env("FJ_SHIM_TOKEN", "secret-token")
        .env("FJ_SHIM_REAL_GH", &real_gh)
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
        .env_clear()
        .env("HOME", fixture.home())
        .env("PATH", fixture.bin())
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
fn fixture_exposes_isolated_paths_and_private_bin_only() -> TestResult {
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
    let paths = std::env::split_paths(path_value).collect::<Vec<_>>();

    assert_eq!(paths, vec![fixture.bin().to_path_buf()]);
    assert!(fixture
        .bin()
        .join(format!("git{}", std::env::consts::EXE_SUFFIX))
        .is_file());
    #[cfg(unix)]
    assert!(fixture.bin().join("git").is_symlink());
    fixture.init_git_repo()?;
    Ok(())
}

#[test]
fn harness_loads_phase_01_compatibility_contract() -> TestResult {
    let contract = load_contract()?;

    assert_eq!(contract.schema, "gh-forgejo-shim.compatibility-contract.v2");
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

/// A fake real gh whose `--version` prints `first_line`, in a directory of its own.
fn gh_reporting(
    fixture: &CliFixture,
    name: &str,
    first_line: &str,
) -> io::Result<std::path::PathBuf> {
    let dir = fixture.root().join(name);
    fs::create_dir_all(&dir)?;
    let path = dir.join("gh");
    write_script_from_child(
        &path,
        &format!("#!/bin/sh\nprintf '%s\\n' '{first_line}'\n"),
    )?;
    Ok(path)
}

fn bootstrap_with_gh(
    fixture: &CliFixture,
    real_gh: &OsStr,
    env: &[(&str, &str)],
) -> io::Result<(Option<i32>, String)> {
    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", real_gh)
        .env("FJ_SHIM_TOKEN", "bootstrap-token")
        .envs(env.iter().copied())
        .args(["bootstrap", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

#[test]
fn bootstrap_without_a_supported_gh_changes_nothing() -> TestResult {
    // D3: no real gh, an older one, a pre-release of the minimum, or one whose version can't be
    // read: bootstrap says why and what to install, and neither allowlists nor installs.
    for (name, first_line, said) in [
        ("missing", None, "could not find the real GitHub CLI"),
        (
            "older",
            Some("gh version 2.103.2 (2026-04-01)"),
            "found gh 2.103.2",
        ),
        (
            "pre",
            Some("gh version 2.103.3-rc.1"),
            "found gh 2.103.3-rc.1",
        ),
        ("odd", Some("hub version 2.14.2"), "not `gh version`"),
        ("dev", Some("gh version DEV"), "a development build"),
    ] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        let real_gh = match first_line {
            Some(line) => gh_reporting(&fixture, name, line)?.into_os_string(),
            None => "/missing/gh".into(),
        };
        let (code, stdout) = bootstrap_with_gh(&fixture, &real_gh, &[])?;
        assert_eq!(code, Some(1), "{name}: {stdout}");
        assert!(
            stdout.contains("[fix] real gh:") && stdout.contains(said),
            "{name}: {stdout}"
        );
        assert!(stdout.contains("gh 2.103.3 or newer"), "{name}: {stdout}");
        assert!(
            stdout.contains("https://github.com/cli/cli#installation"),
            "{name}: {stdout}"
        );
        assert!(
            !fixture.bin().join("gh").exists(),
            "{name}: installed the shim"
        );
        assert_eq!(shim_config(&fixture)?, "", "{name}: wrote the config");
    }
    Ok(())
}

#[test]
fn bootstrap_accepts_the_minimum_gh_and_newer() -> TestResult {
    for first_line in [
        "gh version 2.103.3 (2026-05-01)",
        "gh version 2.104.0-rc.1 (2026-06-01)",
        "gh version 3.0.0",
    ] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        let real_gh = gh_reporting(&fixture, "real", first_line)?;
        let (_, stdout) = bootstrap_with_gh(&fixture, real_gh.as_os_str(), &[])?;
        assert!(
            stdout.contains("[ok] real gh: found gh "),
            "{first_line}: {stdout}"
        );
        assert!(fixture.bin().join("gh").exists(), "{first_line}: {stdout}");
    }
    Ok(())
}

#[test]
fn the_opt_in_covers_an_unreadable_gh_but_never_an_older_one() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let dev = gh_reporting(&fixture, "dev", "gh version DEV")?;
    let (_, stdout) = bootstrap_with_gh(
        &fixture,
        dev.as_os_str(),
        &[("FJ_SHIM_ALLOW_UNKNOWN_GH", "1")],
    )?;
    assert!(stdout.contains("[warn] real gh:"), "{stdout}");
    assert!(
        stdout.contains("FJ_SHIM_ALLOW_UNKNOWN_GH is set"),
        "{stdout}"
    );
    assert!(fixture.bin().join("gh").exists(), "{stdout}");

    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let older = gh_reporting(&fixture, "older", "gh version 2.90.0")?;
    let (code, stdout) = bootstrap_with_gh(
        &fixture,
        older.as_os_str(),
        &[("FJ_SHIM_ALLOW_UNKNOWN_GH", "1")],
    )?;
    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains("[fix] real gh: found gh 2.90.0"),
        "{stdout}"
    );
    assert!(!fixture.bin().join("gh").exists(), "{stdout}");
    Ok(())
}

#[test]
fn a_dry_run_with_an_older_gh_reports_it_and_still_shows_the_plan() -> TestResult {
    let fixture = CliFixture::new()?;
    fixture.init_git_repo()?;
    let older = gh_reporting(&fixture, "older", "gh version 2.103.2")?;
    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &older)
        .args(["bootstrap", "--dry-run", "--no-gui-path", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("[fix] real gh: found gh 2.103.2"),
        "{stdout}"
    );
    assert!(stdout.contains("would allowlist"), "{stdout}");
    assert!(!fixture.bin().join("gh").exists(), "{stdout}");
    Ok(())
}

#[test]
fn install_shim_refuses_an_older_gh_and_installs_with_the_minimum() -> TestResult {
    let fixture = CliFixture::new()?;
    let older = gh_reporting(&fixture, "older", "gh version 2.103.2")?;
    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &older)
        .args(["install-shim", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("not installing the gh shim"), "{stderr}");
    assert!(
        stderr.contains("found gh 2.103.2") && stderr.contains("2.103.3"),
        "{stderr}"
    );
    assert!(!fixture.bin().join("gh").exists());

    let minimum = gh_reporting(&fixture, "minimum", "gh version 2.103.3 (2026-05-01)")?;
    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &minimum)
        .args(["install-shim", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;
    assert!(output.status.success(), "{output:?}");
    assert!(fixture.bin().join("gh").exists());
    Ok(())
}

#[test]
fn doctor_reports_an_older_gh_and_warns_for_an_allowed_dev_build() -> TestResult {
    let fixture = CliFixture::new()?;
    let older = gh_reporting(&fixture, "older", "gh version 2.103.2")?;
    let older = older.to_string_lossy().into_owned();
    let stdout = doctor(&fixture, &[("FJ_SHIM_REAL_GH", &older)])?;
    assert!(
        stdout.contains("[fix] real gh: found gh 2.103.2"),
        "{stdout}"
    );

    let dev = gh_reporting(&fixture, "dev", "gh version DEV")?;
    let dev = dev.to_string_lossy().into_owned();
    let stdout = doctor(
        &fixture,
        &[("FJ_SHIM_REAL_GH", &dev), ("FJ_SHIM_ALLOW_UNKNOWN_GH", "1")],
    )?;
    assert!(stdout.contains("[warn] real gh:"), "{stdout}");
    Ok(())
}

#[test]
fn install_checks_the_gh_left_once_the_shim_replaces_the_one_found() -> TestResult {
    // The first gh on PATH sits where --force will install the shim, so the wrapper will pass
    // over it and run the next one, which is older: install must check that one (Opus o1#3).
    for command in ["install-shim", "bootstrap"] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        let target_dir = fixture.root().join("target");
        let supported = gh_reporting(&fixture, "target", "gh version 2.103.3")?;
        let older = gh_reporting(&fixture, "later", "gh version 2.90.0")?;
        let path = std::env::join_paths([
            target_dir.as_path(),
            older.parent().ok_or("no parent")?,
            fixture.bin(),
        ])?;
        let mut args = vec![command, "--force"];
        if command == "bootstrap" {
            args.push("--no-gui-path");
        }
        args.push("--bin-dir");
        let output = fixture
            .command("gfj")?
            .env_remove("FJ_SHIM_REAL_GH")
            .env("PATH", &path)
            .env("FJ_SHIM_TOKEN", "bootstrap-token")
            .args(&args)
            .arg(&target_dir)
            .output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(1), "{command}: {text}");
        assert!(text.contains("found gh 2.90.0"), "{command}: {text}");
        assert_eq!(
            fs::read_to_string(&supported)?,
            "#!/bin/sh\nprintf '%s\\n' 'gh version 2.103.3'\n",
            "{command} replaced the gh it found"
        );
    }
    Ok(())
}

#[test]
fn install_shim_force_refuses_an_older_gh() -> TestResult {
    let fixture = CliFixture::new()?;
    let older = gh_reporting(&fixture, "older", "gh version 2.103.2")?;
    fs::write(fixture.bin().join("gh"), "unrelated")?;
    let output = fixture
        .command("gfj")?
        .env("FJ_SHIM_REAL_GH", &older)
        .args(["install-shim", "--force", "--bin-dir"])
        .arg(fixture.bin())
        .output()?;
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(fs::read_to_string(fixture.bin().join("gh"))?, "unrelated");
    Ok(())
}

#[cfg(unix)]
#[test]
fn install_passes_over_a_link_to_the_gh_it_replaces() -> TestResult {
    // PATH=alias:target:later, with alias/gh a link to target/gh: once the shim replaces
    // target/gh, the link reaches the shim, so the wrapper runs later/gh (Sol s1#1).
    for command in ["install-shim", "bootstrap"] {
        let fixture = CliFixture::new()?;
        fixture.init_git_repo()?;
        let target_dir = fixture.root().join("target");
        let supported = gh_reporting(&fixture, "target", "gh version 2.103.3")?;
        let older = gh_reporting(&fixture, "later", "gh version 2.90.0")?;
        let alias = fixture.root().join("alias");
        fs::create_dir(&alias)?;
        std::os::unix::fs::symlink(&supported, alias.join("gh"))?;
        let path = std::env::join_paths([
            alias.as_path(),
            target_dir.as_path(),
            older.parent().ok_or("no parent")?,
            fixture.bin(),
        ])?;
        let mut args = vec![command, "--force"];
        if command == "bootstrap" {
            args.push("--no-gui-path");
        }
        args.push("--bin-dir");
        let output = fixture
            .command("gfj")?
            .env_remove("FJ_SHIM_REAL_GH")
            .env("PATH", &path)
            .env("FJ_SHIM_TOKEN", "bootstrap-token")
            .args(&args)
            .arg(&target_dir)
            .output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(1), "{command}: {text}");
        assert!(text.contains("found gh 2.90.0"), "{command}: {text}");
    }
    Ok(())
}
