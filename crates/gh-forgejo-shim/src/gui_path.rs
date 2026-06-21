//! macOS GUI PATH LaunchAgent support.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::EnvMap;
use crate::{Result, ShimError};

pub const LABEL: &str = "com.gh-forgejo-shim.user-gui-path";
pub const PLIST_NAME: &str = "com.gh-forgejo-shim.user-gui-path.plist";
pub const LAUNCHCTL_PATH: &str = "/bin/launchctl";

const SYSTEM_GUI_PATH_DIRS: &[&str] = &[
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
pub struct GuiPathResult {
    pub plist_path: PathBuf,
    pub path_value: String,
    pub applied: bool,
    pub apply_error: Option<String>,
}

pub fn default_plist_path(home: Option<&Path>) -> PathBuf {
    home_dir(home, &EnvMap::new())
        .join("Library")
        .join("LaunchAgents")
        .join(PLIST_NAME)
}

pub fn default_gui_path(home: Option<&Path>, env: &EnvMap) -> String {
    let root = home_dir(home, env);
    let candidates = std::iter::once(root.join(".local").join("bin").display().to_string())
        .chain(SYSTEM_GUI_PATH_DIRS.iter().map(|path| (*path).to_string()))
        .collect::<Vec<_>>();
    dedupe(candidates).join(":")
}

pub fn launch_agent_plist(path_value: &str) -> String {
    let escaped_path = escape_xml_text(path_value);
    vec![
        r#"<?xml version="1.0" encoding="UTF-8"?>"#.to_string(),
        r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN""#.to_string(),
        r#"  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#.to_string(),
        r#"<plist version="1.0">"#.to_string(),
        "<dict>".to_string(),
        "  <key>Label</key>".to_string(),
        format!("  <string>{LABEL}</string>"),
        String::new(),
        "  <key>ProgramArguments</key>".to_string(),
        "  <array>".to_string(),
        "    <string>/bin/launchctl</string>".to_string(),
        "    <string>setenv</string>".to_string(),
        "    <string>PATH</string>".to_string(),
        format!("    <string>{escaped_path}</string>"),
        "  </array>".to_string(),
        String::new(),
        "  <key>RunAtLoad</key>".to_string(),
        "  <true/>".to_string(),
        "</dict>".to_string(),
        "</plist>".to_string(),
        String::new(),
    ]
    .join("\n")
}

pub fn install_gui_path(
    path_value: Option<&str>,
    plist_path: Option<&Path>,
    home: Option<&Path>,
    env: &EnvMap,
    apply_now: bool,
    launchctl_path: Option<&Path>,
) -> Result<GuiPathResult> {
    let value = path_value
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_gui_path(home, env));
    let target = plist_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_plist_path(home));

    let Some(parent) = target.parent() else {
        return Err(ShimError::new(format!(
            "could not determine parent directory for {}",
            target.display()
        )));
    };
    fs::create_dir_all(parent)?;
    fs::write(&target, launch_agent_plist(&value))?;

    if !apply_now {
        return Ok(GuiPathResult {
            plist_path: target,
            path_value: value,
            applied: false,
            apply_error: None,
        });
    }

    let (applied, apply_error) = apply_gui_path_with_launchctl(
        &value,
        launchctl_path.unwrap_or_else(|| Path::new(LAUNCHCTL_PATH)),
    );
    Ok(GuiPathResult {
        plist_path: target,
        path_value: value,
        applied,
        apply_error,
    })
}

pub fn uninstall_gui_path(plist_path: Option<&Path>, home: Option<&Path>) -> Result<PathBuf> {
    let target = plist_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_plist_path(home));
    match fs::remove_file(&target) {
        Ok(()) => Ok(target),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(target),
        Err(error) => Err(ShimError::new(format!(
            "could not remove {}: {error}",
            target.display()
        ))),
    }
}

pub fn apply_gui_path(path_value: &str) -> (bool, Option<String>) {
    apply_gui_path_with_launchctl(path_value, Path::new(LAUNCHCTL_PATH))
}

pub fn apply_gui_path_with_launchctl(
    path_value: &str,
    launchctl_path: &Path,
) -> (bool, Option<String>) {
    let output = match Command::new(launchctl_path)
        .arg("setenv")
        .arg("PATH")
        .arg(path_value)
        .output()
    {
        Ok(output) => output,
        Err(error) => return (false, Some(error.to_string())),
    };
    if output.status.success() {
        return (true, None);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return (false, Some(stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return (false, Some(stdout));
    }
    (
        false,
        Some(format!(
            "launchctl exited {}",
            output.status.code().unwrap_or(1)
        )),
    )
}

pub fn current_launchd_path() -> Option<String> {
    current_launchd_path_with_launchctl(Path::new(LAUNCHCTL_PATH))
}

pub fn current_launchd_path_with_launchctl(launchctl_path: &Path) -> Option<String> {
    let output = Command::new(launchctl_path)
        .arg("getenv")
        .arg("PATH")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub fn path_contains_dir(path_value: &str, directory: &Path) -> bool {
    let target = directory.display().to_string();
    path_value.split(':').any(|part| part == target)
}

fn home_dir(home: Option<&Path>, env: &EnvMap) -> PathBuf {
    home.map(Path::to_path_buf)
        .or_else(|| env.get("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for value in values {
        if !value.is_empty() && seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

fn escape_xml_text(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn default_gui_path_prioritizes_user_local_and_homebrew() {
        let value = default_gui_path(Some(Path::new("/Users/alice")), &EnvMap::new());

        let parts = value.split(':').collect::<Vec<_>>();
        assert_eq!(parts.first(), Some(&"/Users/alice/.local/bin"));
        assert!(parts.contains(&"/opt/homebrew/bin"), "{value}");
        assert!(parts.contains(&"/usr/bin"), "{value}");
    }

    #[test]
    fn launch_agent_plist_sets_path_with_launchctl() {
        let plist = launch_agent_plist("/tmp/a&b:/usr/bin");

        assert!(plist.contains(LABEL), "{plist}");
        assert!(plist.contains("<string>/bin/launchctl</string>"), "{plist}");
        assert!(plist.contains("<string>setenv</string>"), "{plist}");
        assert!(plist.contains("<string>PATH</string>"), "{plist}");
        assert!(plist.contains("/tmp/a&amp;b:/usr/bin"), "{plist}");
    }

    #[test]
    fn install_and_uninstall_gui_path_plist() -> Result<()> {
        let root = temp_root()?;
        let target = root.join("LaunchAgents").join("test.plist");
        let result = install_gui_path(
            Some("/tmp/bin:/usr/bin"),
            Some(&target),
            Some(&root),
            &EnvMap::new(),
            false,
            None,
        )?;

        assert_eq!(result.plist_path, target);
        assert!(!result.applied);
        let text = fs::read_to_string(&target)?;
        assert!(text.contains("/tmp/bin:/usr/bin"), "{text}");

        let removed = uninstall_gui_path(Some(&target), Some(&root))?;

        assert_eq!(removed, target);
        assert!(!removed.exists());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn apply_gui_path_uses_injected_launchctl() -> Result<()> {
        let root = temp_root()?;
        let launchctl = root.join("launchctl");
        let log = root.join("launchctl.log");
        write_executable(
            &launchctl,
            &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n", log.display()),
        )?;

        let (applied, error) = apply_gui_path_with_launchctl("/tmp/bin:/usr/bin", &launchctl);

        assert!(applied, "{error:?}");
        assert_eq!(error, None);
        assert_eq!(
            fs::read_to_string(log)?.trim(),
            "setenv PATH /tmp/bin:/usr/bin"
        );
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn apply_gui_path_reports_launchctl_error() -> Result<()> {
        let root = temp_root()?;
        let launchctl = root.join("launchctl");
        write_executable(&launchctl, "#!/bin/sh\necho denied >&2\nexit 42\n")?;

        let (applied, error) = apply_gui_path_with_launchctl("/tmp/bin:/usr/bin", &launchctl);

        assert!(!applied);
        assert_eq!(error.as_deref(), Some("denied"));
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    #[test]
    fn uninstall_refuses_to_remove_directory() -> Result<()> {
        let root = temp_root()?;
        let target = root.join("Library").join("LaunchAgents").join(PLIST_NAME);
        fs::create_dir_all(&target)?;

        let result = uninstall_gui_path(Some(&target), Some(&root));

        assert!(result.is_err());
        assert!(target.is_dir());
        fs::remove_dir_all(root).ok();
        Ok(())
    }

    fn temp_root() -> Result<PathBuf> {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gh-forgejo-shim-gui-path-test-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&path).map_err(|error| ShimError::new(error.to_string()))?;
        Ok(path)
    }

    fn write_executable(path: &Path, contents: &str) -> io::Result<()> {
        fs::write(path, contents)?;
        make_executable(path)
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
