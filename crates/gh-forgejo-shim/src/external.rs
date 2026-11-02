//! External command discovery and delegation helpers.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::shim::is_managed_shim;

pub const DEFAULT_FALLBACK_DIRS: &[&str] = &[
    "~/.local/bin",
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRunResult {
    pub code: i32,
    pub message: String,
}

pub fn find_program(
    name: &str,
    configured: Option<&Path>,
    env: &HashMap<String, String>,
) -> Option<PathBuf> {
    let fallback_dirs = DEFAULT_FALLBACK_DIRS
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    find_program_with_fallbacks(name, configured, env, &fallback_dirs)
}

pub fn find_program_with_fallbacks(
    name: &str,
    configured: Option<&Path>,
    env: &HashMap<String, String>,
    fallback_dirs: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(configured) = configured.filter(|path| !path.as_os_str().is_empty()) {
        let path = expand_home(configured, env);
        if is_executable_file(&path) {
            if name == "gh" && should_skip_gh_candidate(&path) {
                // A configured managed shim is ignored so a real gh in PATH can still win.
            } else {
                return Some(path);
            }
        } else {
            return None;
        }
    }

    for directory in candidate_dirs(env, fallback_dirs) {
        let candidate = directory.join(name);
        if !is_executable_file(&candidate) {
            continue;
        }
        if name == "gh" && should_skip_gh_candidate(&candidate) {
            continue;
        }
        return Some(candidate);
    }
    None
}

/// Runs `path` with exactly `env`, which may hold values that aren't UTF-8.
pub fn run_program_capture<K, V>(
    path: &Path,
    argv: &[OsString],
    env: impl IntoIterator<Item = (K, V)>,
    cwd: Option<&Path>,
) -> io::Result<ProgramOutput>
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let output = command_with_env(path, argv, env, cwd).output()?;
    Ok(ProgramOutput {
        code: exit_code(output.status),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Runs `path` on this process's terminal with exactly `env`, which may hold values that aren't
/// UTF-8: pass `std::env::vars_os()` to hand on the environment byte for byte.
pub fn run_program_inherit<K, V>(
    path: &Path,
    argv: &[OsString],
    env: impl IntoIterator<Item = (K, V)>,
    cwd: Option<&Path>,
) -> io::Result<i32>
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let status = command_with_env(path, argv, env, cwd).status()?;
    Ok(inherited_exit_code(status))
}

/// The status a shell reports for a child: its exit code, or 128 plus the signal that killed it.
fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

thread_local! {
    /// The signal that killed the last child this thread waited on in the foreground, or 0.
    static CHILD_SIGNAL: Cell<i32> = const { Cell::new(0) };
}

/// [`exit_code`] for a child this process stands in for, such as real gh or real git. A signal
/// that killed it is remembered, so [`exit_like_child`] can end this process with the same
/// signal.
pub fn inherited_exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;

        CHILD_SIGNAL.set(status.signal().unwrap_or(0));
    }
    exit_code(status)
}

/// Returns `code`, unless it is the one [`inherited_exit_code`] gave for a child killed by a
/// signal on this thread. Then this process dies of that signal too, so its parent sees the death
/// that the child's parent saw. Call it last, once traces are written.
///
/// signal-hook puts the signal's default action back and unblocks it first: Rust ignores SIGPIPE
/// and handles SIGSEGV and SIGBUS itself, and a parent may have left the signal ignored or
/// blocked. It returns without raising for a signal missing from its table (Linux real-time
/// ones) or listed there as ignored (SIGIO, which terminates on Linux), so those are raised as
/// they are. An init process (pid 1, also inside a PID namespace such as a container) never gets
/// a signal it sends itself under the default action, and signal-hook would then abort, so it
/// returns `code` instead. So does any process that survives.
pub fn exit_like_child(code: i32) -> i32 {
    #[cfg(unix)]
    {
        let signal = CHILD_SIGNAL.get();
        if signal != 0 && code == 128 + signal && std::process::id() != 1 {
            let _ = signal_hook::low_level::emulate_default_handler(signal);
            let _ = signal_hook::low_level::raise(signal);
        }
    }
    code
}

pub fn git_output(args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub fn git_run(args: &[&str], cwd: Option<&Path>) -> GitRunResult {
    let mut command = Command::new("git");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            return GitRunResult {
                code: 127,
                message: error.to_string(),
            };
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    GitRunResult {
        code: output.status.code().unwrap_or(1),
        message: if stderr.is_empty() { stdout } else { stderr },
    }
}

pub fn open_web_url(url: &str) {
    #[cfg(test)]
    {
        OPENED_WEB_URLS.with(|urls| urls.borrow_mut().push(url.to_string()));
    }
    #[cfg(not(test))]
    {
        let mut command = if cfg!(target_os = "macos") {
            let mut command = Command::new("open");
            command.arg(url);
            command
        } else if cfg!(target_os = "windows") {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", "", url]);
            command
        } else {
            let mut command = Command::new("xdg-open");
            command.arg(url);
            command
        };
        let _ = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
pub fn take_opened_web_urls() -> Vec<String> {
    OPENED_WEB_URLS.with(|urls| std::mem::take(&mut *urls.borrow_mut()))
}

#[cfg(test)]
thread_local! {
    /// The URLs this test thread asked to open. Per thread, not per process: tests run in
    /// parallel, and one that opens a URL must never see, or take, another's (2ny).
    static OPENED_WEB_URLS: std::cell::RefCell<Vec<String>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn command_with_env<K, V>(
    path: &Path,
    argv: &[OsString],
    env: impl IntoIterator<Item = (K, V)>,
    cwd: Option<&Path>,
) -> Command
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let mut command = Command::new(path);
    command.args(argv).env_clear().envs(env);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
}

fn candidate_dirs(env: &HashMap<String, String>, fallback_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = Vec::<PathBuf>::new();
    let mut result = Vec::<PathBuf>::new();

    let path_dirs = env
        .get("PATH")
        .map(|value| std::env::split_paths(value).collect::<Vec<_>>())
        .unwrap_or_default();

    for directory in path_dirs.into_iter().chain(fallback_dirs.iter().cloned()) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let normalized = expand_home(&directory, env);
        if seen.iter().any(|existing| existing == &normalized) {
            continue;
        }
        seen.push(normalized.clone());
        result.push(normalized);
    }

    result
}

/// `~` and `~/…` under `HOME`; any other path, and any path when `HOME` is unset, as it is. For
/// the launcher to share, so a configured `~/gh` expands one way everywhere (n2e). It compares bytes,
/// not path components: the rest after `~/` is joined exactly as written, so `~//gh` is still
/// `/gh` and `~/gh/` keeps its slash, and bytes that aren't UTF-8 stay as they are.
#[cfg(unix)]
pub(crate) fn expand_home(path: &Path, env: &HashMap<String, String>) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;

    let Some(home) = env.get("HOME") else {
        return path.to_path_buf();
    };
    let bytes = path.as_os_str().as_bytes();
    if bytes == b"~" {
        return PathBuf::from(home);
    }
    match bytes.strip_prefix(b"~/") {
        Some(rest) => Path::new(home).join(OsStr::from_bytes(rest)),
        None => path.to_path_buf(),
    }
}

#[cfg(not(unix))]
pub(crate) fn expand_home(path: &Path, env: &HashMap<String, String>) -> PathBuf {
    let value = path.as_os_str().to_string_lossy();
    if value == "~" {
        return env
            .get("HOME")
            .map_or_else(|| path.to_path_buf(), PathBuf::from);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = env.get("HOME") {
            return Path::new(home).join(rest);
        }
    }
    path.to_path_buf()
}

fn should_skip_gh_candidate(path: &Path) -> bool {
    is_managed_shim(path) || is_current_executable(path)
}

/// Compiled into both entry points of every build from here on, so discovery recognizes a copy
/// of either without knowing where it came from. `#[used]` keeps the bytes in the binary in one
/// piece, even where the optimizer would otherwise fold the comparison into the code.
#[used]
static DISPATCHER_MARKER: [u8; 47] = *b"gh-forgejo-shim dispatcher, not real gh: 7f3c1e";

/// Whether `path` may run this dispatcher: the running binary under any name (a symlink or a
/// hardlink), or a binary within a tenth of its size that carries [`DISPATCHER_MARKER`], such as
/// a copy of either entry point (`gfj` and `gh-forgejo-shim` differ in bytes). Some copies still
/// pass, and the launcher's own recursion guards cover them: a build from before the marker, a
/// marked build of another size (a debug `gfj` beside a release dispatcher), or a release whose
/// size moved more than a tenth.
fn is_current_executable(path: &Path) -> bool {
    // On Linux the running binary stays readable here even after its file is deleted.
    let running = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        match std::env::current_exe() {
            Ok(current) => current,
            Err(_) => return false,
        }
    };
    let (Ok(candidate_meta), Ok(running_meta)) = (path.metadata(), running.metadata()) else {
        return false;
    };
    same_file(path, &candidate_meta, &running, &running_meta)
        || is_copy(path, &candidate_meta, &running_meta)
}

/// A dispatcher build of about the running binary's size. Real gh is a different size, so it
/// costs only the `stat` already made. A candidate in range that can't be read counts as a copy:
/// it may still be executable, and it can't be shown to differ.
fn is_copy(candidate: &Path, candidate_meta: &fs::Metadata, running_meta: &fs::Metadata) -> bool {
    let (len, running_len) = (candidate_meta.len(), running_meta.len());
    if len.abs_diff(running_len) > running_len / 10 {
        return false;
    }
    let Ok(bytes) = fs::read(candidate) else {
        return true;
    };
    bytes
        .windows(DISPATCHER_MARKER.len())
        .any(|window| window == DISPATCHER_MARKER)
}

/// Whether both paths reach one file, through symlinks or as hardlinks.
#[cfg(unix)]
fn same_file(_: &Path, left: &fs::Metadata, _: &Path, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    (left.dev(), left.ino()) == (right.dev(), right.ino())
}

#[cfg(not(unix))]
fn same_file(left: &Path, _: &fs::Metadata, right: &Path, _: &fs::Metadata) -> bool {
    let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize()) else {
        return false;
    };
    left == right
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    metadata.is_file() && is_executable(&metadata.permissions())
}

#[cfg(unix)]
fn is_executable(permissions: &std::fs::Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;

    permissions.mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_permissions: &std::fs::Permissions) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[cfg(unix)]
    #[test]
    fn expand_home_takes_tilde_and_tilde_slash_only_and_keeps_every_byte() {
        use std::os::unix::ffi::OsStrExt;

        let env = HashMap::from([("HOME".to_string(), "/home/me".to_string())]);
        // Compared as bytes: `Path` equality would hide a dropped trailing `/` or `.`.
        for (path, expanded) in [
            ("~", "/home/me"),
            ("~/", "/home/me/"),
            ("~/bin/gh", "/home/me/bin/gh"),
            ("~//gh", "/gh"),
            ("~/gh/", "/home/me/gh/"),
            ("~/./gh", "/home/me/./gh"),
            ("~/gh/.", "/home/me/gh/."),
            ("~user/gh", "~user/gh"),
            ("/opt/~/gh", "/opt/~/gh"),
            ("bin/gh", "bin/gh"),
        ] {
            assert_eq!(
                expand_home(Path::new(path), &env).as_os_str().as_bytes(),
                expanded.as_bytes(),
                "{path}"
            );
        }
        // Not UTF-8 after `~/`: kept byte for byte, not replaced with U+FFFD.
        let odd = Path::new(OsStr::from_bytes(b"~/caf\xff/gh"));
        assert_eq!(
            expand_home(odd, &env).as_os_str().as_bytes(),
            b"/home/me/caf\xff/gh"
        );
        assert_eq!(
            expand_home(Path::new("~/gh"), &HashMap::new()),
            Path::new("~/gh"),
            "HOME unset"
        );
    }

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn finds_real_gh_in_fallback_dirs_when_path_is_minimal() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let minimal_path = fixture.root.join("minimal").join("bin");
        fs::create_dir_all(&minimal_path)?;
        let fallback = fixture.root.join("homebrew").join("bin");
        let real_gh = fixture.fake_executable(&fallback, "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([
            ("PATH".to_string(), minimal_path.display().to_string()),
            ("HOME".to_string(), fixture.root.display().to_string()),
        ]);

        let found = find_program_with_fallbacks("gh", None, &env, &[fallback]);

        assert_eq!(found, Some(real_gh));
        Ok(())
    }

    #[test]
    fn skips_managed_gh_shim_in_fallback_dirs() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let shim_dir = fixture.root.join("local").join("bin");
        let real_dir = fixture.root.join("homebrew").join("bin");
        fixture.fake_executable(
            &shim_dir,
            "gh",
            "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
        )?;
        let real_gh = fixture.fake_executable(&real_dir, "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([
            ("PATH".to_string(), String::new()),
            ("HOME".to_string(), fixture.root.display().to_string()),
        ]);

        let found = find_program_with_fallbacks("gh", None, &env, &[shim_dir, real_dir]);

        assert_eq!(found, Some(real_gh));
        Ok(())
    }

    #[test]
    fn configured_path_wins_over_fallback_dirs() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let configured = fixture.fake_executable(
            &fixture.root.join("configured"),
            "gh",
            "#!/bin/sh\nexit 0\n",
        )?;
        let fallback =
            fixture.fake_executable(&fixture.root.join("fallback"), "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([("PATH".to_string(), String::new())]);
        let fallback_dir = fallback
            .parent()
            .ok_or_else(|| io::Error::other("fallback executable has no parent"))?
            .to_path_buf();

        let found = find_program_with_fallbacks("gh", Some(&configured), &env, &[fallback_dir]);

        assert_eq!(found, Some(configured));
        Ok(())
    }

    #[test]
    fn configured_managed_gh_shim_does_not_win() -> io::Result<()> {
        let fixture = Fixture::new()?;
        let configured = fixture.fake_executable(
            &fixture.root.join("configured"),
            "gh",
            "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
        )?;
        let fallback =
            fixture.fake_executable(&fixture.root.join("fallback"), "gh", "#!/bin/sh\nexit 0\n")?;
        let env = HashMap::from([("PATH".to_string(), String::new())]);
        let fallback_dir = fallback
            .parent()
            .ok_or_else(|| io::Error::other("fallback executable has no parent"))?
            .to_path_buf();

        let found = find_program_with_fallbacks("gh", Some(&configured), &env, &[fallback_dir]);

        assert_eq!(found, Some(fallback));
        Ok(())
    }

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> io::Result<Self> {
            let root = std::env::temp_dir().join(format!(
                "gh-forgejo-shim-external-{}-{}",
                std::process::id(),
                NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root)?;
            Ok(Self { root })
        }

        fn fake_executable(&self, root: &Path, name: &str, contents: &str) -> io::Result<PathBuf> {
            fs::create_dir_all(root)?;
            let path = root.join(name);
            fs::write(&path, contents)?;
            make_executable(&path)?;
            Ok(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }
}
