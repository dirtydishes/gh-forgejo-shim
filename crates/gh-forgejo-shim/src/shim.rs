//! Managed shim lifecycle and detection used by real-`gh` discovery.

use std::fs::{self, File};
use std::io::Read;
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

pub fn install_shim(
    bin_dir: Option<&Path>,
    home: Option<&Path>,
    executable: &Path,
    force: bool,
) -> Result<PathBuf> {
    let target = shim_path(bin_dir, home)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    if target.exists() && !is_managed_shim(&target) && !force {
        return Err(ShimError::new(format!(
            "{} exists and is not managed by gh-forgejo-shim; pass --force to overwrite",
            target.display()
        )));
    }
    fs::write(&target, shim_script(executable))?;
    make_executable(&target)?;
    Ok(target)
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
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
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
