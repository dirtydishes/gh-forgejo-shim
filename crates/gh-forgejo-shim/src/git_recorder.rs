//! Temporary git recorder support.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::trace::{append_jsonl, is_sensitive_key, redact_text_with};
use crate::{shim, Result, ShimError};

pub const MARKER: &str = "managed by gh-forgejo-shim git recorder";
pub const TRACE_PATH_ENV: &str = "FJ_SHIM_TRACE";
pub const TRACE_ENV: &str = TRACE_PATH_ENV;
pub const TRACE_BODY_ENV: &str = "FJ_SHIM_TRACE_BODY";
pub const REAL_GIT_ENV: &str = "FJ_SHIM_REAL_GIT";

const SCHEMA: &str = "gh-forgejo-shim.git-recorder.v1";
const MARKER_FILE: &str = ".gh-forgejo-shim-git-recorder";
// Recorded in the marker file when the recorder made its directory, so removal may delete it.
const CREATED_DIRECTORY: &str = "created directory";
const SECRET_REPLACEMENT: &str = "[REDACTED]";
const EXCERPT_LIMIT: usize = 4096;
// The recorder's own files carry their marker in the first few lines.
const MAX_HEAD_BYTES: u64 = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRecorder {
    pub wrapper_dir: PathBuf,
    pub wrapper_path: PathBuf,
    pub trace_path: PathBuf,
    pub real_git: PathBuf,
    pub path: String,
    pub env: HashMap<String, String>,
}

pub fn create_git_recorder(
    trace_path: &Path,
    real_git: Option<&Path>,
    root_dir: Option<&Path>,
    wrapper_dir: Option<&Path>,
    env: &HashMap<String, String>,
    recorder_program: &Path,
) -> Result<GitRecorder> {
    let git_path = resolve_real_git(real_git, env)?;
    let (directory, created) = make_wrapper_dir(root_dir, wrapper_dir)?;
    let wrapper = directory.join("git");
    let marker = directory.join(MARKER_FILE);
    // `symlink_metadata` so a dangling symlink counts as a file the recorder doesn't own.
    for (path, header) in [(&wrapper, wrapper_header()), (&marker, marker_header())] {
        if fs::symlink_metadata(path).is_ok() && !written_by_recorder(path, &header) {
            return Err(ShimError::new(format!(
                "{} exists and is not managed by gh-forgejo-shim",
                path.display()
            )));
        }
    }

    let trace = absolute_path(trace_path)?;
    let real_git = absolute_path(&git_path)?;
    let script = git_recorder_script(recorder_program, &real_git, &trace);
    shim::replace_file(&wrapper, script.as_bytes(), true)?;
    let created = created || created_directory(&marker);
    let marker_text = if created {
        format!("{}{CREATED_DIRECTORY}\n", marker_header())
    } else {
        marker_header()
    };
    shim::replace_file(&marker, marker_text.as_bytes(), false)?;

    let path = prepend_path(&directory, env.get("PATH").map_or("", String::as_str));
    let mut launch_env = env.clone();
    launch_env.insert("PATH".to_string(), path.clone());
    launch_env.insert(TRACE_PATH_ENV.to_string(), trace.display().to_string());
    launch_env.insert(REAL_GIT_ENV.to_string(), real_git.display().to_string());

    Ok(GitRecorder {
        wrapper_dir: directory,
        wrapper_path: wrapper,
        trace_path: trace,
        real_git,
        path,
        env: launch_env,
    })
}

pub fn remove_git_recorder(path: &Path) -> Result<PathBuf> {
    let mut directory = expand_home(path);
    if directory.is_file() {
        directory = directory
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ShimError::new("git recorder path has no parent directory"))?;
    }
    if !directory.exists() {
        return Ok(directory);
    }

    // Remove only what the recorder wrote, and only when both files are ones it writes. The
    // directory may be one the user already had, such as ~/bin, so it goes only if the recorder
    // made it and nothing else is left in it.
    let wrapper = directory.join("git");
    let marker = directory.join(MARKER_FILE);
    if !written_by_recorder(&wrapper, &wrapper_header())
        || !written_by_recorder(&marker, &marker_header())
    {
        return Err(ShimError::new(format!(
            "{} is not a gh-forgejo-shim git recorder directory",
            directory.display()
        )));
    }
    let created = created_directory(&marker);
    remove_recorder_file(&wrapper, &wrapper_header())?;
    remove_recorder_file(&marker, &marker_header())?;
    if created {
        match fs::remove_dir(&directory) {
            Err(error) if error.kind() != io::ErrorKind::DirectoryNotEmpty => {
                return Err(error.into())
            }
            _ => {}
        }
    }
    Ok(directory)
}

/// Removes `path` after checking it again: a name checked earlier can have been replaced since.
/// A replacement in the moment between this check and the unlink can't be ruled out, since
/// POSIX removes files by name, but anyone who can make one can delete that file themselves.
fn remove_recorder_file(path: &Path, header: &str) -> Result<()> {
    if !written_by_recorder(path, header) {
        return Err(ShimError::new(format!(
            "{} changed during removal; it was left in place",
            path.display()
        )));
    }
    fs::remove_file(path)?;
    Ok(())
}

fn wrapper_header() -> String {
    format!("#!/bin/sh\n# {MARKER}\nexec ")
}

fn marker_header() -> String {
    format!("{MARKER}\n")
}

/// Whether `path` is a regular file, not a symlink, that starts the way the recorder writes it.
fn written_by_recorder(path: &Path, header: &str) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
        && read_head(path).is_some_and(|bytes| bytes.starts_with(header.as_bytes()))
}

fn created_directory(marker: &Path) -> bool {
    read_head(marker).is_some_and(|bytes| {
        let text = String::from_utf8_lossy(&bytes);
        text.contains(MARKER) && text.lines().any(|line| line == CREATED_DIRECTORY)
    })
}

/// The first bytes of `path`, following symlinks. Bytes, not text: a check that fails on
/// non-UTF-8 content would let a recorder pass as something else.
fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_HEAD_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

pub fn git_recorder_script(recorder_program: &Path, real_git: &Path, trace_path: &Path) -> String {
    format!(
        "#!/bin/sh\n# {MARKER}\nexec {} trace git-recorder run --real-git {} --trace {} -- \"$@\"\n",
        shell_quote(&recorder_program.display().to_string()),
        shell_quote(&real_git.display().to_string()),
        shell_quote(&trace_path.display().to_string())
    )
}

pub fn run_git_recorder(real_git: &Path, trace_path: &Path, args: &[String]) -> i32 {
    let started = Instant::now();
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from(""));
    let mut command = Command::new(real_git);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut record = base_record(args, &cwd);
    let include_excerpt = env::var(TRACE_BODY_ENV).ok().as_deref() == Some("1");

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message =
                format!("gh-forgejo-shim git recorder: unable to run real git: {error}\n");
            let _ = io::stderr().write_all(message.as_bytes());
            finish_record(
                &mut record,
                FinishedGit {
                    started,
                    exit_code: 127,
                    stdout_bytes: 0,
                    stderr_bytes: message.len(),
                    stdout_excerpt: &[],
                    stderr_excerpt: message.as_bytes(),
                    include_excerpt,
                },
            );
            let _ = append_jsonl(trace_path, &record);
            return 127;
        }
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_forwarder = stdout.map(|stream| forward_stdout(stream, include_excerpt));
    let stderr_forwarder = stderr.map(|stream| forward_stderr(stream, include_excerpt));

    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let message =
                format!("gh-forgejo-shim git recorder: unable to wait for real git: {error}\n");
            let _ = io::stderr().write_all(message.as_bytes());
            finish_record(
                &mut record,
                FinishedGit {
                    started,
                    exit_code: 127,
                    stdout_bytes: 0,
                    stderr_bytes: message.len(),
                    stdout_excerpt: &[],
                    stderr_excerpt: message.as_bytes(),
                    include_excerpt,
                },
            );
            let _ = append_jsonl(trace_path, &record);
            return 127;
        }
    };
    let exit_code = status.code().unwrap_or(1);
    let stdout_stats = stdout_forwarder
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();
    let stderr_stats = stderr_forwarder
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();

    finish_record(
        &mut record,
        FinishedGit {
            started,
            exit_code,
            stdout_bytes: stdout_stats.bytes,
            stderr_bytes: stderr_stats.bytes,
            stdout_excerpt: &stdout_stats.excerpt,
            stderr_excerpt: &stderr_stats.excerpt,
            include_excerpt,
        },
    );
    let _ = append_jsonl(trace_path, &record);
    exit_code
}

#[derive(Debug, Default)]
struct StreamStats {
    bytes: usize,
    excerpt: Vec<u8>,
}

fn forward_stdout(stream: ChildStdout, include_excerpt: bool) -> thread::JoinHandle<StreamStats> {
    thread::spawn(move || {
        let stdout = io::stdout();
        forward_stream(stream, stdout.lock(), include_excerpt)
    })
}

fn forward_stderr(stream: ChildStderr, include_excerpt: bool) -> thread::JoinHandle<StreamStats> {
    thread::spawn(move || {
        let stderr = io::stderr();
        forward_stream(stream, stderr.lock(), include_excerpt)
    })
}

fn forward_stream<R, W>(mut source: R, mut destination: W, include_excerpt: bool) -> StreamStats
where
    R: Read,
    W: Write,
{
    let mut stats = StreamStats::default();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => break,
        };
        stats.bytes += read;
        if include_excerpt && stats.excerpt.len() < EXCERPT_LIMIT {
            let remaining = EXCERPT_LIMIT - stats.excerpt.len();
            stats
                .excerpt
                .extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if destination.write_all(&buffer[..read]).is_err() {
            continue;
        }
        let _ = destination.flush();
    }
    stats
}

fn base_record(args: &[String], cwd: &Path) -> Value {
    let argv = redact_argv(args);
    let command_argv = std::iter::once("git".to_string())
        .chain(argv.iter().cloned())
        .collect::<Vec<_>>();
    json!({
        "schema": SCHEMA,
        "kind": "git",
        "timestamp": utc_timestamp(),
        "cwd": redact_text(&cwd.display().to_string()),
        "argv": argv,
        "command_argv": command_argv,
    })
}

struct FinishedGit<'a> {
    started: Instant,
    exit_code: i32,
    stdout_bytes: usize,
    stderr_bytes: usize,
    stdout_excerpt: &'a [u8],
    stderr_excerpt: &'a [u8],
    include_excerpt: bool,
}

fn finish_record(record: &mut Value, finished: FinishedGit<'_>) {
    let duration = finished.started.elapsed().as_secs_f64();
    record["duration"] = json!(round_float(duration, 6));
    record["duration_ms"] = json!(round_float(duration * 1000.0, 3));
    record["exit_code"] = json!(finished.exit_code);
    record["stdout_bytes"] = json!(finished.stdout_bytes);
    record["stderr_bytes"] = json!(finished.stderr_bytes);
    record["stdout"] = json!({"bytes": finished.stdout_bytes});
    record["stderr"] = json!({"bytes": finished.stderr_bytes});
    if finished.include_excerpt {
        let stdout_text = redact_text(&String::from_utf8_lossy(finished.stdout_excerpt));
        let stderr_text = redact_text(&String::from_utf8_lossy(finished.stderr_excerpt));
        record["stdout_excerpt"] = json!(stdout_text);
        record["stderr_excerpt"] = json!(stderr_text);
        record["stdout"]["excerpt"] = record["stdout_excerpt"].clone();
        record["stderr"]["excerpt"] = record["stderr_excerpt"].clone();
    }
}

fn resolve_real_git(real_git: Option<&Path>, env: &HashMap<String, String>) -> Result<PathBuf> {
    let path = if let Some(path) = real_git {
        expand_home(path)
    } else if let Some(value) = env
        .get(REAL_GIT_ENV)
        .filter(|value| !value.trim().is_empty())
    {
        expand_home(Path::new(value))
    } else {
        find_on_path("git", env.get("PATH").map_or("", String::as_str))
            .ok_or_else(|| ShimError::new(format!("could not find git; set {REAL_GIT_ENV}")))?
    };
    // A wrapper that runs a recorder as its real git can end up running itself without bound.
    // Requiring an existing file also rules out the new wrapper's own path, however it's
    // spelled: that file either doesn't exist yet or is a recorder.
    if !is_executable_file(&path) {
        return Err(ShimError::new(format!(
            "{} is not an executable file; pass the real git with --real-git or {REAL_GIT_ENV}",
            path.display()
        )));
    }
    if is_recorder_wrapper(&path) {
        return Err(not_real_git(&path));
    }
    Ok(path)
}

fn not_real_git(path: &Path) -> ShimError {
    ShimError::new(format!(
        "{} is a gh-forgejo-shim git recorder, not the real git; pass the real git with --real-git or {REAL_GIT_ENV}",
        path.display()
    ))
}

/// Finds `name` on PATH, skipping git recorders: a recorder directory is often on PATH already.
fn find_on_path(name: &str, path_value: &str) -> Option<PathBuf> {
    env::split_paths(path_value).find_map(|directory| {
        let candidate = directory.join(name);
        (is_executable_file(&candidate) && !is_recorder_wrapper(&candidate)).then_some(candidate)
    })
}

/// Returns the wrapper directory, and whether this call created it.
fn make_wrapper_dir(
    root_dir: Option<&Path>,
    wrapper_dir: Option<&Path>,
) -> Result<(PathBuf, bool)> {
    if let Some(path) = wrapper_dir {
        let directory = absolute_path(path)?;
        let existed = directory.exists();
        fs::create_dir_all(&directory)?;
        return Ok((directory, !existed));
    }

    let root = root_dir.map_or_else(env::temp_dir, expand_home);
    fs::create_dir_all(&root)?;
    for attempt in 0..1000 {
        let directory = root.join(format!(
            "gh-forgejo-shim-git-{}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok((absolute_path(&directory)?, true)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ShimError::new(
        "unable to allocate temporary git recorder directory",
    ))
}

fn prepend_path(directory: &Path, path_value: &str) -> String {
    if path_value.is_empty() {
        return directory.display().to_string();
    }
    match env::join_paths([directory.as_os_str(), std::ffi::OsStr::new(path_value)]) {
        Ok(value) => value.to_string_lossy().into_owned(),
        Err(_) => format!("{}:{path_value}", directory.display()),
    }
}

/// Whether `path`, following symlinks, mentions the recorder marker. It refuses recorders as
/// real git, so it matches more loosely than `written_by_recorder`.
fn is_recorder_wrapper(path: &Path) -> bool {
    read_head(path).is_some_and(|bytes| {
        bytes
            .windows(MARKER.len())
            .any(|window| window == MARKER.as_bytes())
    })
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    let expanded = expand_home(path);
    if expanded.is_absolute() {
        return Ok(expanded);
    }
    Ok(env::current_dir()?.join(expanded))
}

fn expand_home(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if value == "~" {
        return env::var_os("HOME").map_or_else(|| path.to_path_buf(), PathBuf::from);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | ':')
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn redact_argv(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            redacted.push(SECRET_REPLACEMENT.to_string());
            redact_next = false;
            continue;
        }

        let (key, separator, value) = split_assignment(arg);
        if key.starts_with('-') && is_sensitive_key(key) {
            if separator.is_some() {
                redacted.push(format!("{key}={SECRET_REPLACEMENT}"));
            } else {
                redacted.push(key.to_string());
                redact_next = true;
            }
            continue;
        }
        if separator.is_some() && is_sensitive_key(key) {
            redacted.push(format!("{key}={SECRET_REPLACEMENT}"));
            continue;
        }
        if separator.is_some() {
            redacted.push(format!("{key}={}", redact_text(value)));
        } else {
            redacted.push(redact_text(arg));
        }
    }
    redacted
}

fn split_assignment(value: &str) -> (&str, Option<char>, &str) {
    value
        .find('=')
        .map(|index| (&value[..index], Some('='), &value[index + 1..]))
        .unwrap_or((value, None, ""))
}

fn redact_text(value: &str) -> String {
    redact_text_with(value, SECRET_REPLACEMENT)
}

fn utc_timestamp() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("{}.{:03}Z", duration.as_secs(), duration.subsec_millis()),
        Err(_) => "0.000Z".to_string(),
    }
}

fn round_float(value: f64, digits: i32) -> f64 {
    let factor = 10_f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_argv_values() {
        let argv = redact_argv(&[
            "fetch".to_string(),
            "--password=argv-secret".to_string(),
            "--token".to_string(),
            "next-argv-secret".to_string(),
            "https://user:password@example.com/repo.git".to_string(),
        ]);

        assert_eq!(
            argv,
            vec![
                "fetch",
                "--password=[REDACTED]",
                "--token",
                "[REDACTED]",
                "https://[REDACTED]@example.com/repo.git",
            ]
        );
    }

    #[test]
    fn generated_script_contains_managed_marker() {
        let script = git_recorder_script(
            Path::new("/tmp/gfj"),
            Path::new("/usr/bin/git"),
            Path::new("/tmp/trace.jsonl"),
        );

        assert!(script.contains(MARKER));
        assert!(script.contains("trace git-recorder run"));
    }

    /// A new directory for one test. It never reuses or clears an existing one.
    fn recorder_root(name: &str) -> Result<PathBuf> {
        for attempt in 0..1000 {
            let root = env::temp_dir().join(format!(
                "gfj-recorder-{name}-{}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Ok(root),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ShimError::new("unable to allocate a test directory"))
    }

    fn create(
        root: &Path,
        root_dir: Option<&Path>,
        wrapper_dir: Option<&Path>,
    ) -> Result<GitRecorder> {
        let real_git = root.join("real-git");
        fs::write(&real_git, "#!/bin/sh\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&real_git, fs::Permissions::from_mode(0o755))?;
        }
        create_git_recorder(
            &root.join("trace.jsonl"),
            Some(&real_git),
            root_dir,
            wrapper_dir,
            &HashMap::new(),
            Path::new("/tmp/gfj"),
        )
    }

    #[test]
    fn remove_deletes_the_directory_it_created() -> Result<()> {
        let root = recorder_root("created")?;
        let recorder = create(&root, Some(&root), None)?;

        remove_git_recorder(&recorder.wrapper_dir)?;

        assert!(!recorder.wrapper_dir.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn remove_keeps_a_directory_that_already_existed() -> Result<()> {
        let root = recorder_root("existing")?;
        let directory = root.join("bin");
        fs::create_dir(&directory)?;
        create(&root, None, Some(&directory))?;

        remove_git_recorder(&directory)?;

        assert!(directory.exists());
        assert_eq!(fs::read_dir(&directory)?.count(), 0);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn recreating_keeps_the_created_directory_removable() -> Result<()> {
        let root = recorder_root("recreated")?;
        let directory = root.join("recorder");
        create(&root, None, Some(&directory))?;
        create(&root, None, Some(&directory))?;

        remove_git_recorder(&directory)?;

        assert!(!directory.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn remove_refuses_files_the_recorder_did_not_write() -> Result<()> {
        let root = recorder_root("spoofed")?;
        let directory = root.join("bin");
        fs::create_dir(&directory)?;
        let script = directory.join("git");
        fs::write(&script, format!("#!/bin/sh\n# mentions {MARKER}\n"))?;
        // A valid marker, so only the wrapper check stands between the script and removal.
        fs::write(directory.join(MARKER_FILE), marker_header())?;
        assert!(remove_git_recorder(&directory).is_err());
        assert!(script.exists());

        let recorder = create(&root, Some(&root), None)?;
        fs::remove_file(&script)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&recorder.wrapper_path, &script)?;
        assert!(remove_git_recorder(&directory).is_err());
        assert!(recorder.wrapper_path.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn create_never_writes_through_a_symlink() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = recorder_root("symlinks")?;
        let precious = root.join("precious");
        fs::write(&precious, "precious\n")?;
        for name in ["git", MARKER_FILE] {
            let directory = root.join(format!("bin-{}", name.trim_start_matches('.')));
            fs::create_dir(&directory)?;
            let refused = |root: &Path| {
                create(root, None, Some(&directory))
                    .is_err_and(|error| error.message().contains("not managed by gh-forgejo-shim"))
            };
            symlink(&precious, directory.join(name))?;
            assert!(refused(&root));
            let dangling = root.join(format!("missing-{}", name.trim_start_matches('.')));
            fs::remove_file(directory.join(name))?;
            symlink(&dangling, directory.join(name))?;
            assert!(refused(&root));
            assert!(!dangling.exists());
        }
        assert_eq!(fs::read_to_string(&precious)?, "precious\n");
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn refuses_a_recorder_as_real_git() -> Result<()> {
        let root = recorder_root("refuses")?;
        let first = create(&root, Some(&root), None)?;
        let directory = root.join("second");
        let refused = |real_git: &Path| {
            create_git_recorder(
                &root.join("trace.jsonl"),
                Some(real_git),
                None,
                Some(&directory),
                &HashMap::new(),
                Path::new("/tmp/gfj"),
            )
            .is_err_and(|error| error.message().contains("the real git"))
        };

        assert!(refused(&first.wrapper_path));
        // Non-UTF-8 bytes must not hide a recorder, including as its own real git.
        let mut script = fs::read(&first.wrapper_path)?;
        script.extend_from_slice(b"#\xff\n");
        fs::write(&first.wrapper_path, script)?;
        assert!(refused(&first.wrapper_path));
        let own = create_git_recorder(
            &root.join("trace.jsonl"),
            Some(&first.wrapper_path),
            None,
            Some(&first.wrapper_dir),
            &HashMap::new(),
            Path::new("/tmp/gfj"),
        );
        assert!(own.is_err_and(|error| error.message().contains("not the real git")));
        assert!(refused(&directory.join("git")));
        assert!(refused(&directory.join("..").join("second").join("git")));
        assert!(
            !directory.exists(),
            "a refused create left its directory behind"
        );
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn forward_stream_copies_output_and_collects_limited_excerpt() {
        let input = vec![b'a'; EXCERPT_LIMIT + 3];
        let mut output = Vec::new();

        let stats = forward_stream(input.as_slice(), &mut output, true);

        assert_eq!(stats.bytes, EXCERPT_LIMIT + 3);
        assert_eq!(stats.excerpt.len(), EXCERPT_LIMIT);
        assert_eq!(output.len(), EXCERPT_LIMIT + 3);
        assert!(stats.excerpt.iter().all(|byte| *byte == b'a'));
    }
}
