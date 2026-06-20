//! Managed shim detection used by real-`gh` discovery.

use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const MARKER: &str = "managed by gh-forgejo-shim";
const MAX_MARKER_BYTES: u64 = 8192;

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
        && (text.contains("gh-forgejo-shim gh") || text.contains("-m gh_forgejo_shim gh"))
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
}
