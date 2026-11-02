//! The launcher's private run directory (launcher spec §4).
//!
//! One launched call owns one `gfj-<16 hex>` directory, mode 0700. A lock file, held for the
//! launcher's lifetime, tells a later launch's sweep whether the owner is still alive: the
//! kernel releases the lock when the process dies, SIGKILL included, so pid reuse can't fool it.

use std::collections::HashMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The lock file's whole content. A sweep removes nothing from a directory whose lock lacks it.
pub const MARKER: &str = "gh-forgejo-shim run directory v1\n";

/// Every name a launcher creates in its run directory. Removal is by these names only, never
/// recursive, so a directory holding anything else survives (audit D1).
pub const NAMES: [&str; 7] = [
    "s",
    "config.yml",
    "hosts.yml",
    "credentials",
    "gh",
    "panic",
    "lock",
];

const PREFIX: &str = "gfj-";
const SOCKET: &str = "s";

/// `sun_path` size, terminating NUL included.
#[cfg(target_os = "macos")]
const SUN_PATH: usize = 104;
#[cfg(not(target_os = "macos"))]
const SUN_PATH: usize = 108;

/// The parent a launch uses, and the variable it came from, for error messages.
pub fn parent(env: &HashMap<String, String>) -> (PathBuf, &'static str) {
    for name in ["XDG_RUNTIME_DIR", "TMPDIR"] {
        if let Some(dir) = env.get(name).filter(|dir| !dir.is_empty()) {
            return (PathBuf::from(dir), name);
        }
    }
    (PathBuf::from("/tmp"), "TMPDIR")
}

/// Every parent any launch could have used, each existing one once, for the sweep.
pub fn sweep_parents(env: &HashMap<String, String>) -> Vec<PathBuf> {
    let mut parents: Vec<PathBuf> = Vec::new();
    let candidates = ["XDG_RUNTIME_DIR", "TMPDIR"]
        .iter()
        .filter_map(|name| env.get(*name).filter(|dir| !dir.is_empty()))
        .map(PathBuf::from)
        .chain([PathBuf::from("/tmp")]);
    for candidate in candidates {
        let Ok(canonical) = fs::canonicalize(&candidate) else {
            continue;
        };
        if canonical.is_dir() && !parents.contains(&canonical) {
            parents.push(canonical);
        }
    }
    parents
}

/// Checks a parent before anything is created: gh hands the run directory's path to `sh`
/// through Go's `%q`, which isn't shell quoting, and the socket path must fit `sun_path`.
pub fn check_parent(parent: &Path, source: &str) -> Result<(), String> {
    let text = parent.to_str().unwrap_or("");
    if text.is_empty() || !text.bytes().all(path_byte_allowed) {
        return Err(format!(
            "{source} ({}) has characters the launcher can't use; it allows only ASCII letters, digits, '/', '.', '_' and '-'",
            parent.display()
        ));
    }
    let socket = text.trim_end_matches('/').len() + 1 + PREFIX.len() + 16 + 1 + SOCKET.len();
    if socket + 1 > SUN_PATH {
        return Err(format!(
            "{source} ({}) is too long for the launcher's socket path ({socket} bytes, the limit is {})",
            parent.display(),
            SUN_PATH - 1
        ));
    }
    Ok(())
}

fn path_byte_allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
}

/// A live run directory. Its lock is held until [`RunDir::remove`] or drop.
#[derive(Debug)]
pub struct RunDir {
    path: PathBuf,
    lock: Option<File>,
}

impl RunDir {
    /// Creates the directory and takes its lock, in spec §4's order: `mkdir`, then a lock file
    /// created under a temporary name, locked and marked before it's renamed to `lock`. A
    /// sweeper opens only `lock`, so it never finds an unlocked one in a live directory.
    pub fn create(parent: &Path) -> io::Result<Self> {
        let path = loop {
            let candidate = parent.join(format!("{PREFIX}{}", random_hex(8)?));
            match DirBuilder::new().mode(0o700).create(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };
        let staged = path.join(format!(".lock-{}", random_hex(8)?));
        let lock = (|| {
            let mut lock = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&staged)?;
            lock.try_lock().map_err(|error| match error {
                fs::TryLockError::Error(error) => error,
                fs::TryLockError::WouldBlock => {
                    io::Error::other("a fresh lock file was already locked")
                }
            })?;
            lock.write_all(MARKER.as_bytes())?;
            fs::rename(&staged, path.join("lock"))?;
            Ok(lock)
        })();
        match lock {
            Ok(lock) => Ok(Self {
                path,
                lock: Some(lock),
            }),
            Err(error) => {
                let _ = fs::remove_file(&staged);
                let _ = fs::remove_dir(&path);
                Err(error)
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn socket_path(&self) -> PathBuf {
        self.path.join(SOCKET)
    }

    /// Writes one of the run directory's files. It must not exist yet.
    pub fn write(&self, name: &str, contents: &[u8], mode: u32) -> io::Result<PathBuf> {
        debug_assert!(NAMES.contains(&name), "{name} is not a run directory name");
        let path = self.path.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)?;
        file.write_all(contents)?;
        Ok(path)
    }

    /// Removes every known name, then the directory, then releases the lock. `lock` goes last,
    /// so a token is never on disk without it. Unknown files keep the directory.
    pub fn remove(mut self) -> io::Result<()> {
        let result = remove_known(&self.path);
        self.lock = None;
        result
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        if self.lock.is_some() {
            let _ = remove_known(&self.path);
        }
    }
}

fn remove_known(dir: &Path) -> io::Result<()> {
    for name in NAMES {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::remove_dir(dir)
}

/// Removes run directories whose launcher is dead, under every parent. Never fails or delays a
/// launch: every error skips the entry.
pub fn sweep(parents: &[PathBuf]) {
    let uid = rustix::process::getuid().as_raw();
    for parent in parents {
        let Ok(entries) = fs::read_dir(parent) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if is_run_dir_name(name) {
                sweep_one(&entry.path(), uid);
            }
        }
    }
}

fn is_run_dir_name(name: &str) -> bool {
    name.strip_prefix(PREFIX).is_some_and(|hex| {
        hex.len() == 16
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn sweep_one(dir: &Path, uid: u32) {
    let Ok(metadata) = fs::symlink_metadata(dir) else {
        return;
    };
    if !metadata.is_dir() || metadata.uid() != uid {
        return;
    }
    let Ok(mut lock) = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(dir.join("lock"))
    else {
        return;
    };
    if lock.try_lock().is_err() {
        return;
    }
    let mut content = String::new();
    if lock.read_to_string(&mut content).is_err() || content != MARKER {
        return;
    }
    let _ = remove_known(dir);
}

/// `bytes` random bytes from the OS, as lowercase hex.
pub fn random_hex(bytes: usize) -> io::Result<String> {
    let mut buffer = vec![0; bytes];
    File::open("/dev/urandom")?.read_exact(&mut buffer)?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gfj-test-{}", random_hex(8).expect("random")));
        fs::create_dir(&dir).expect("scratch dir");
        dir
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn create_makes_a_private_locked_directory() {
        let parent = scratch();
        let run = RunDir::create(&parent).expect("create");
        let name = run
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("name");
        assert!(is_run_dir_name(name), "{name}");
        assert_eq!(mode(run.path()), 0o700);
        assert_eq!(
            fs::read_to_string(run.path().join("lock")).expect("lock"),
            MARKER
        );
        let entries: Vec<_> = fs::read_dir(run.path()).expect("list").flatten().collect();
        assert_eq!(
            entries.len(),
            1,
            "only `lock`, no staged name left: {entries:?}"
        );
        run.remove().expect("remove");
        fs::remove_dir(&parent).expect("parent left empty");
    }

    #[test]
    fn remove_takes_only_known_names() {
        let parent = scratch();
        let run = RunDir::create(&parent).expect("create");
        run.write("hosts.yml", b"x", 0o600).expect("hosts");
        assert_eq!(mode(&run.path().join("hosts.yml")), 0o600);
        assert!(run.write("hosts.yml", b"y", 0o600).is_err(), "create_new");
        let path = run.path().to_path_buf();
        fs::write(path.join("unknown"), b"keep").expect("unknown");
        assert!(run.remove().is_err(), "rmdir fails on the unknown file");
        assert!(path.join("unknown").exists());
        assert!(!path.join("hosts.yml").exists());
        assert!(!path.join("lock").exists());
        fs::remove_file(path.join("unknown")).expect("cleanup");
        fs::remove_dir(&path).expect("cleanup");
        fs::remove_dir(&parent).expect("cleanup");
    }

    #[test]
    fn sweep_removes_only_dead_marked_directories() {
        let parent = scratch();
        let live = RunDir::create(&parent).expect("live");
        live.write("hosts.yml", b"token", 0o600).expect("hosts");

        let dead = parent.join("gfj-0123456789abcdef");
        fs::create_dir(&dead).expect("dead");
        fs::write(dead.join("lock"), MARKER).expect("lock");
        fs::write(dead.join("hosts.yml"), b"token").expect("hosts");

        let unmarked = parent.join("gfj-fedcba9876543210");
        fs::create_dir(&unmarked).expect("unmarked");
        fs::write(unmarked.join("lock"), b"something else\n").expect("lock");

        let no_lock = parent.join("gfj-00000000000000aa");
        fs::create_dir(&no_lock).expect("no lock");

        let wrong_name = parent.join("gfj-backup");
        fs::create_dir(&wrong_name).expect("wrong name");
        fs::write(wrong_name.join("lock"), MARKER).expect("lock");

        let linked = parent.join("gfj-00000000000000bb");
        std::os::unix::fs::symlink(&dead, &linked).expect("symlink");

        sweep(std::slice::from_ref(&parent));

        assert!(!dead.exists(), "a dead, marked directory goes");
        assert!(live.path().join("hosts.yml").exists(), "a live one stays");
        assert!(unmarked.join("lock").exists(), "no marker, untouched");
        assert!(no_lock.exists(), "no lock, untouched");
        assert!(wrong_name.join("lock").exists(), "wrong name, untouched");
        assert!(
            fs::symlink_metadata(&linked).is_ok(),
            "a symlink is never followed"
        );

        live.remove().expect("remove live");
        fs::remove_file(&linked).expect("cleanup");
        for dir in [&unmarked, &wrong_name] {
            fs::remove_file(dir.join("lock")).expect("cleanup");
        }
        for dir in [&unmarked, &no_lock, &wrong_name] {
            fs::remove_dir(dir).expect("cleanup");
        }
        fs::remove_dir(&parent).expect("cleanup");
    }

    #[test]
    fn drop_cleans_up_like_remove() {
        let parent = scratch();
        let path = {
            let run = RunDir::create(&parent).expect("create");
            run.write("credentials", b"secret", 0o600)
                .expect("credentials");
            run.path().to_path_buf()
        };
        assert!(!path.exists());
        fs::remove_dir(&parent).expect("parent left empty");
    }

    #[test]
    fn parent_prefers_runtime_dir_then_tmpdir() {
        let mut env = HashMap::new();
        assert_eq!(parent(&env), (PathBuf::from("/tmp"), "TMPDIR"));
        env.insert("TMPDIR".to_string(), "/t".to_string());
        assert_eq!(parent(&env), (PathBuf::from("/t"), "TMPDIR"));
        env.insert("XDG_RUNTIME_DIR".to_string(), String::new());
        assert_eq!(parent(&env), (PathBuf::from("/t"), "TMPDIR"));
        env.insert("XDG_RUNTIME_DIR".to_string(), "/run/user/1".to_string());
        assert_eq!(
            parent(&env),
            (PathBuf::from("/run/user/1"), "XDG_RUNTIME_DIR")
        );
    }

    #[test]
    fn check_parent_rejects_shell_characters_and_long_paths() {
        assert!(check_parent(Path::new("/var/folders/ab/c_d-e.f/T/"), "TMPDIR").is_ok());
        for bad in ["/tmp/a b", "/tmp/$x", "/tmp/a'b", "/tmp/é", ""] {
            let error = check_parent(Path::new(bad), "TMPDIR").expect_err(bad);
            assert!(error.starts_with("TMPDIR"), "{error}");
        }
        let long = format!(
            "/{}",
            "a".repeat(SUN_PATH - 1 - 1 - PREFIX.len() - 16 - 2 - 1)
        );
        assert!(
            check_parent(Path::new(&long), "TMPDIR").is_ok(),
            "exactly fits"
        );
        let longer = format!("{long}a");
        assert!(
            check_parent(Path::new(&longer), "TMPDIR").is_err(),
            "one byte over"
        );
    }
}
