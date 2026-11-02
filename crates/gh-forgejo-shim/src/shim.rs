//! Managed shim lifecycle and detection used by real-`gh` discovery.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::{Result, ShimError};

pub const MARKER: &str = "managed by gh-forgejo-shim";
const RUST_WRAPPER_MARKER: &str = "rust gh-forgejo-shim wrapper";
const MAX_MARKER_BYTES: u64 = 8192;

pub fn default_bin_dir(home: Option<&Path>) -> Result<PathBuf> {
    let home = home.ok_or_else(|| ShimError::new("HOME is required to find ~/.local/bin"))?;
    Ok(home.join(".local").join("bin"))
}

pub fn shim_path(bin_dir: Option<&Path>, home: Option<&Path>) -> Result<PathBuf> {
    Ok(match bin_dir {
        Some(bin_dir) => bin_dir.join("gh"),
        None => default_bin_dir(home)?.join("gh"),
    })
}

pub fn shim_script(executable: &Path) -> String {
    format!(
        "#!/bin/sh\n# {MARKER}\n# rollback-compatible marker: gh-forgejo-shim gh\n# {RUST_WRAPPER_MARKER}\nexec {} gh \"$@\"\n",
        shell_quote_path(executable)
    )
}

pub fn is_managed_shim(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.take(MAX_MARKER_BYTES).read_to_end(&mut bytes).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes);
    text.contains(MARKER)
        && (text.contains("gh-forgejo-shim gh")
            || text.contains("-m gh_forgejo_shim gh")
            || (text.contains(RUST_WRAPPER_MARKER) && text.contains(" gh \"$@\"")))
}

/// `fs::write`, except that a file it creates gets `mode` (which umask can only narrow) instead of
/// 0666 & ~umask, under which a group member could edit it. An existing file keeps its mode: it's
/// the user's to choose. It writes in place, through a symlink at `path`; use [`replace_file`] for
/// anything that runs, or that must never be seen half-written.
pub(crate) fn write_file(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    options.open(path)?.write_all(contents)
}

/// `fs::create_dir_all`, except that each directory it creates gets `mode` (which umask can only
/// narrow) instead of 0777 & ~umask. Under umask 002 a group-writable directory would let another
/// account in the group rename its own file over one the shim wrote, such as the `gh` wrapper.
/// Directories that already exist keep their mode: they're the user's to choose.
pub(crate) fn create_dirs(path: &Path, mode: u32) -> std::io::Result<()> {
    dir_builder(mode).recursive(true).create(path)
}

/// `fs::create_dir` with `mode`, as [`create_dirs`]. It fails if `path` exists, so a caller
/// picking a fresh name can't reuse a directory someone else planted.
pub(crate) fn create_dir(path: &Path, mode: u32) -> std::io::Result<()> {
    dir_builder(mode).create(path)
}

fn dir_builder(mode: u32) -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    builder
}

pub fn install_shim(
    bin_dir: Option<&Path>,
    home: Option<&Path>,
    executable: &Path,
    force: bool,
) -> Result<PathBuf> {
    let target = shim_path(bin_dir, home)?;
    if let Some(parent) = target.parent() {
        create_dirs(parent, 0o755)?;
    }
    // `symlink_metadata` so a dangling `gh` symlink counts as an existing, unmanaged target.
    if fs::symlink_metadata(&target).is_ok() && !is_managed_shim(&target) && !force {
        return Err(ShimError::new(format!(
            "{} exists and is not managed by gh-forgejo-shim; pass --force to overwrite",
            target.display()
        )));
    }
    replace_file(&target, shim_script(executable).as_bytes(), true)?;
    Ok(target)
}

/// Writes `contents` to a new file beside `target`, then renames it over `target`. The rename
/// replaces a symlink at `target` itself, where writing to the path would follow it and overwrite
/// whatever it points to, such as the real gh.
pub(crate) fn replace_file(target: &Path, contents: &[u8], executable: bool) -> Result<()> {
    // An executable starts owner-only: a handle another account opened while it was still
    // 0666 & ~umask would keep write access through make_executable and the rename. Other
    // files keep the default mode they've always had.
    let (mut file, temp) = create_temp_file(target, if executable { 0o600 } else { 0o666 })?;
    let result = (|| -> Result<()> {
        file.write_all(contents)?;
        if executable {
            make_executable(&file)?;
        }
        file.sync_all()?;
        // Another writer in the directory could have swapped the temporary name for a link.
        if !is_same_file(&file, &temp) {
            return Err(ShimError::new(format!(
                "{} changed while it was being written",
                temp.display()
            )));
        }
        fs::rename(&temp, target)?;
        Ok(())
    })();
    // Clean up only a name that still holds this file: one swapped in by someone else stays.
    if result.is_err() && is_same_file(&file, &temp) {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Creates a new file beside `target` with `mode`, which umask can only narrow.
fn create_temp_file(target: &Path, mode: u32) -> Result<(File, PathBuf)> {
    let parent = target.parent().unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    for attempt in 0..1000 {
        let temp = parent.join(format!(
            ".{name}.gh-forgejo-shim-{}-{attempt}",
            std::process::id()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        match options.open(&temp) {
            Ok(file) => return Ok((file, temp)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ShimError::new(format!(
                    "could not create a temporary file in {}: {error}",
                    parent.display()
                )))
            }
        }
    }
    Err(ShimError::new(format!(
        "unable to create a temporary file in {}",
        parent.display()
    )))
}

pub fn uninstall_shim(bin_dir: Option<&Path>, home: Option<&Path>) -> Result<PathBuf> {
    let target = shim_path(bin_dir, home)?;
    if !target.exists() {
        return Ok(target);
    }
    if !is_managed_shim(&target) {
        return Err(ShimError::new(format!(
            "{} is not managed by gh-forgejo-shim; refusing to remove",
            target.display()
        )));
    }
    fs::remove_file(&target)?;
    Ok(target)
}

fn shell_quote_path(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
    {
        return value.to_string();
    }
    let mut quoted = String::from("'");
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(unix)]
fn make_executable(file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Set, not added: the new file starts at 0666 & ~umask, which can include group write.
    file.set_permissions(fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn is_same_file(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn is_same_file(_file: &File, _path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;

    use super::*;

    #[test]
    fn recognizes_python_managed_shim() -> io::Result<()> {
        let path = std::env::temp_dir().join(format!(
            "gh-forgejo-shim-managed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::write(
            &path,
            "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
        )?;

        assert!(is_managed_shim(&path));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn generated_rust_shim_is_managed_and_rollback_detectable_by_python_rule() -> io::Result<()> {
        let script = shim_script(Path::new("/tmp/bin with spaces/gh-forgejo-shim"));

        assert!(script.contains("# managed by gh-forgejo-shim"));
        assert!(script.contains("gh-forgejo-shim gh"));
        assert!(script.contains("exec '/tmp/bin with spaces/gh-forgejo-shim' gh \"$@\""));

        let path = unique_path("gh-forgejo-shim-rust-managed");
        fs::write(&path, script)?;
        assert!(is_managed_shim(&path));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn install_refuses_unmanaged_target_without_force() -> Result<()> {
        let root = unique_path("gh-forgejo-shim-install-refuses");
        fs::create_dir(&root)?;
        let target = root.join("gh");
        fs::write(&target, "#!/bin/sh\n")?;

        let error = install_shim(Some(&root), None, Path::new("/tmp/gh-forgejo-shim"), false)
            .expect_err("unmanaged target should fail");

        assert!(error
            .message()
            .contains("exists and is not managed by gh-forgejo-shim"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn install_force_overwrites_and_uninstall_removes() -> Result<()> {
        let root = unique_path("gh-forgejo-shim-install-force");
        fs::create_dir(&root)?;
        let target = root.join("gh");
        fs::write(&target, "unrelated")?;

        let installed = install_shim(Some(&root), None, Path::new("/tmp/gh-forgejo-shim"), true)?;

        assert_eq!(installed, target);
        assert!(is_managed_shim(&target));
        assert!(!fs::metadata(&target)?.permissions().readonly());

        let removed = uninstall_shim(Some(&root), None)?;
        assert_eq!(removed, target);
        assert!(!target.exists());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn install_force_replaces_a_symlink_without_writing_through_it() -> Result<()> {
        let root = unique_path("gh-forgejo-shim-install-symlink");
        fs::create_dir(&root)?;
        let real = root.join("real-gh");
        fs::write(&real, "real gh\n")?;
        let target = root.join("gh");
        std::os::unix::fs::symlink(&real, &target)?;

        install_shim(Some(&root), None, Path::new("/tmp/gh-forgejo-shim"), true)?;

        assert_eq!(fs::read_to_string(&real)?, "real gh\n");
        assert!(fs::symlink_metadata(&target)?.is_file());
        assert!(is_managed_shim(&target));
        assert_eq!(
            fs::read_dir(&root)?.count(),
            2,
            "a temporary file was left behind"
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn install_refuses_a_dangling_symlink_without_force() -> Result<()> {
        let root = unique_path("gh-forgejo-shim-install-dangling");
        fs::create_dir(&root)?;
        let target = root.join("gh");
        std::os::unix::fs::symlink(root.join("missing"), &target)?;

        let error = install_shim(Some(&root), None, Path::new("/tmp/gh-forgejo-shim"), false)
            .expect_err("a dangling symlink is an unmanaged target");

        assert!(error.message().contains("not managed by gh-forgejo-shim"));
        assert!(fs::symlink_metadata(&target)?.file_type().is_symlink());
        assert!(!root.join("missing").exists());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn uninstall_refuses_unmanaged_target() -> Result<()> {
        let root = unique_path("gh-forgejo-shim-uninstall-refuses");
        fs::create_dir(&root)?;
        fs::write(root.join("gh"), "unrelated")?;

        let error =
            uninstall_shim(Some(&root), None).expect_err("unmanaged target should not be removed");

        assert!(error
            .message()
            .contains("is not managed by gh-forgejo-shim; refusing to remove"));
        assert!(root.join("gh").exists());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn an_executable_is_owner_only_from_creation_and_other_files_keep_the_default() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let root = unique_path("gh-forgejo-shim-temp-mode");
        fs::create_dir(&root)?;

        let (file, temp) = create_temp_file(&root.join("gh"), 0o600)?;
        assert_eq!(file.metadata()?.permissions().mode() & 0o077, 0);
        fs::remove_file(temp)?;

        // A plain file gets what fs::write gives under the same umask, as before.
        replace_file(&root.join("marker"), b"marker\n", false)?;
        fs::write(root.join("reference"), b"reference\n")?;
        let mode =
            |name: &str| fs::metadata(root.join(name)).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(mode("marker")?, mode("reference")?);
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    fn unique_path(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ))
    }
}
