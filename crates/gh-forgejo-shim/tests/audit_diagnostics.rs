//! Adversarial regression tests. All credentials and tools are disposable fakes.
#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use gh_forgejo_shim::{git_recorder, shim};

type TestResult = Result<(), Box<dyn std::error::Error>>;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> io::Result<Self> {
        let root = loop {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("gfj-audit-diagnostics-{}-{id}", std::process::id()));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let fixture = Self {
            home: root.join("home"),
            bin: root.join("tools"),
            root,
        };
        for dir in [
            fixture.home.clone(),
            fixture.bin.clone(),
            fixture.root.join("tmp"),
        ] {
            fs::create_dir_all(dir)?;
        }
        fixture.tool("gh", "#!/bin/sh\nexit 0\n")?;
        fixture.tool("git", "#!/bin/sh\nexit 1\n")?;
        Ok(fixture)
    }

    fn tool(&self, name: &str, body: &str) -> io::Result<PathBuf> {
        let path = self.bin.join(name);
        fs::write(&path, body)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }

    fn env(&self) -> HashMap<String, String> {
        HashMap::from([
            ("HOME".into(), self.home.display().to_string()),
            ("PATH".into(), self.bin.display().to_string()),
            ("TMPDIR".into(), self.root.join("tmp").display().to_string()),
            ("FJ_SHIM_AUTH_STORE".into(), "file".into()),
            ("FJ_SHIM_HOSTS".into(), "forge.example.test".into()),
            (
                "FJ_SHIM_REAL_GH".into(),
                self.bin.join("gh").display().to_string(),
            ),
            ("LC_ALL".into(), "C".into()),
        ])
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gfj"));
        command
            .env_clear()
            .envs(self.env())
            .current_dir(&self.root)
            .stdin(Stdio::null());
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn audit_diag_recorder_remove_preserves_unrelated_files() -> TestResult {
    let f = Fixture::new()?;
    let directory = f.root.join("existing-bin");
    fs::create_dir(&directory)?;
    let unrelated = directory.join("user-tool");
    fs::write(&unrelated, "user-owned tool\n")?;
    git_recorder::create_git_recorder(
        &f.root.join("git.jsonl"),
        Some(&f.bin.join("git")),
        None,
        Some(&directory),
        &f.env(),
        Path::new(env!("CARGO_BIN_EXE_gfj")),
    )?;
    let output = f
        .command()
        .args(["trace", "git-recorder", "remove"])
        .arg(directory.join("git"))
        .output()?;
    assert!(output.status.success());
    assert!(
        unrelated.exists(),
        "removing one git recorder recursively deleted the unrelated user-tool"
    );
    Ok(())
}

#[test]
fn audit_diag_install_refuses_dangling_symlink() -> TestResult {
    let f = Fixture::new()?;
    let target_dir = f.home.join(".local/bin");
    fs::create_dir_all(&target_dir)?;
    let victim = f.home.join("unrelated-file");
    symlink(&victim, target_dir.join("gh"))?;
    let output = f.command().arg("install-shim").output()?;
    assert!(
        !output.status.success() && !victim.exists(),
        "install without --force followed an unmanaged dangling symlink and created its target: exit={:?}, victim_exists={}",
        output.status.code(),
        victim.exists()
    );
    Ok(())
}

#[test]
fn audit_diag_install_drops_other_write_permissions() -> TestResult {
    let f = Fixture::new()?;
    let target_dir = f.home.join(".local/bin");
    fs::create_dir_all(&target_dir)?;
    let target = target_dir.join("gh");
    fs::write(&target, shim::shim_script(Path::new("/old/gfj")))?;
    // The old wrapper's mode must not carry over to the new one.
    fs::set_permissions(&target, fs::Permissions::from_mode(0o666))?;
    // Under umask 000 the new wrapper starts group- and world-writable, so this fails on any
    // runner unless install sets the mode outright.
    let install = Command::new("/bin/sh")
        .args(["-c", "umask 000 && exec \"$0\" install-shim"])
        .arg(env!("CARGO_BIN_EXE_gfj"))
        .env_clear()
        .envs(f.env())
        .current_dir(&f.root)
        .stdin(Stdio::null())
        .output()?;
    assert!(install.status.success(), "install failed: {install:?}");
    let writable_by_others = fs::metadata(target)?.permissions().mode() & 0o022;
    assert_eq!(
        writable_by_others, 0,
        "install made the wrapper executable while preserving group/other write permission"
    );
    Ok(())
}

#[test]
fn audit_diag_created_directories_are_owner_writable_only() -> TestResult {
    let f = Fixture::new()?;
    let fresh_tmp = f.root.join("fresh/tmp");
    // A directory that already exists is the user's, and keeps its mode.
    let existing = f.home.join(".config");
    fs::create_dir(&existing)?;
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o750))?;
    // Under umask 000 a plain create_dir_all makes 0777 directories, so this fails on any
    // runner unless each directory the shim creates gets its own mode.
    let run = |args: &[&str]| -> io::Result<std::process::Output> {
        Command::new("/bin/sh")
            .args(["-c", "umask 000 && exec \"$0\" \"$@\""])
            .arg(env!("CARGO_BIN_EXE_gfj"))
            .args(args)
            .env_clear()
            .envs(f.env())
            .env("TMPDIR", &fresh_tmp)
            .env("FJ_SHIM_TRACE", f.root.join("fresh-trace/gh.jsonl"))
            .current_dir(&f.root)
            .stdin(Stdio::null())
            .output()
    };
    let real_git = f.bin.join("git");
    let trace = f.root.join("fresh-git-trace/git.jsonl");
    let mut commands = vec![
        vec!["install-shim"],
        vec!["gh", "--version"],
        vec!["config", "add-host", "git.example.test"],
        vec![
            "trace",
            "git-recorder",
            "create",
            trace.to_str().ok_or("trace path")?,
            "--real-git",
            real_git.to_str().ok_or("git path")?,
        ],
    ];
    if cfg!(target_os = "macos") {
        commands.push(vec![
            "install-gui-path",
            "--path",
            "/usr/bin:/bin",
            "--no-apply",
        ]);
    }
    for args in commands {
        let output = run(&args)?;
        assert!(output.status.success(), "{args:?} failed: {output:?}");
    }
    let recorder = fs::read_dir(&fresh_tmp)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("gh-forgejo-shim-git-"))
        })
        .ok_or("no recorder directory")?;
    // Running the recorder's git writes its trace, creating the trace's parent.
    let mut git = Command::new("/bin/sh");
    git.args(["-c", "umask 000 && exec \"$0\" status"])
        .arg(recorder.join("git"))
        .env_clear()
        .envs(f.env())
        .current_dir(&f.root)
        .stdin(Stdio::null());
    git.output()?;
    let mode = |path: &Path| fs::metadata(path).map(|m| m.permissions().mode() & 0o777);
    let mut created = vec![
        (f.root.join("fresh-trace"), 0o700),
        (f.root.join("fresh-git-trace"), 0o700),
        (f.home.join(".local"), 0o755),
        (f.home.join(".local/bin"), 0o755),
        (f.home.join(".config/gh-forgejo-shim"), 0o700),
        (f.root.join("fresh"), 0o755),
        (fresh_tmp.clone(), 0o755),
        (recorder, 0o755),
    ];
    if cfg!(target_os = "macos") {
        created.push((f.home.join("Library/LaunchAgents"), 0o755));
    }
    assert_eq!(
        mode(&existing)?,
        0o750,
        "an existing directory's mode changed"
    );
    for (path, allowed) in created {
        let mode = mode(&path)?;
        assert_eq!(
            mode & !allowed,
            0,
            "{} was created {mode:o}, more open than {allowed:o}",
            path.display()
        );
    }
    Ok(())
}

#[test]
fn audit_diag_written_config_and_launch_agent_are_owner_writable_only() -> TestResult {
    let f = Fixture::new()?;
    // Under umask 000 a plain fs::write makes 0666 files, so this fails on any runner unless the
    // shim gives each file it creates its own mode.
    let run = |args: &[&str]| -> io::Result<std::process::Output> {
        Command::new("/bin/sh")
            .args(["-c", "umask 000 && exec \"$0\" \"$@\""])
            .arg(env!("CARGO_BIN_EXE_gfj"))
            .args(args)
            .env_clear()
            .envs(f.env())
            .current_dir(&f.root)
            .stdin(Stdio::null())
            .output()
    };
    let mut commands = vec![vec!["config", "add-host", "git.example.test"]];
    let mut written = vec![(f.home.join(".config/gh-forgejo-shim/config.toml"), 0o600)];
    if cfg!(target_os = "macos") {
        commands.push(vec![
            "install-gui-path",
            "--path",
            "/usr/bin:/bin",
            "--no-apply",
        ]);
        written.push((
            f.home
                .join("Library/LaunchAgents")
                .join(gh_forgejo_shim::gui_path::PLIST_NAME),
            0o644,
        ));
    }
    for args in &commands {
        let output = run(args)?;
        assert!(output.status.success(), "{args:?} failed: {output:?}");
    }
    for (path, expected) in &written {
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        assert_eq!(mode, *expected, "{} was written {mode:o}", path.display());
    }

    // An existing file is the user's, and keeps its mode.
    let config = &written[0].0;
    fs::set_permissions(config, fs::Permissions::from_mode(0o640))?;
    let output = run(&["config", "add-host", "other.example.test"])?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::metadata(config)?.permissions().mode() & 0o777, 0o640);
    Ok(())
}

#[test]
fn audit_diag_recorder_recreate_prevents_self_recursion() -> TestResult {
    let f = Fixture::new()?;
    let directory = f.root.join("recorder");
    let first = git_recorder::create_git_recorder(
        &f.root.join("git.jsonl"),
        Some(&f.bin.join("git")),
        None,
        Some(&directory),
        &f.env(),
        Path::new(env!("CARGO_BIN_EXE_gfj")),
    )?;
    let mut env = f.env();
    env.insert("PATH".into(), first.path);
    let second = git_recorder::create_git_recorder(
        &f.root.join("git.jsonl"),
        None,
        None,
        Some(&directory),
        &env,
        Path::new(env!("CARGO_BIN_EXE_gfj")),
    )?;
    assert!(
        second.real_git != second.wrapper_path,
        "recreating the recorder selected itself as real git and rewrote itself to recurse forever"
    );
    Ok(())
}
