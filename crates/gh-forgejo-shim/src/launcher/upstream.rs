//! The config bridge (L4, finding F10): one served host's shim settings, as the gateway's
//! upstream client needs them. The built-in path reads the same settings in
//! `forgejo::ForgejoClient::for_host`; this keeps the launched path to the same rules.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fga_core::map::WipPrefixes;
use url::Url;

use crate::config::{self, Config};

/// A served host's upstream: where its Forgejo lives, which extra CA roots to trust, and which
/// title prefixes mark a draft there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    /// The forge's base URL, ending in `/`: `api_url`, or `https://<host>`.
    pub base: Url,
    /// CA files whose certificates join the normal trust store: the host's `ca_file`, then
    /// `FJ_SHIM_CA_FILE`. Neither turns verification off.
    pub ca_files: Vec<PathBuf>,
    /// The host's `wip_prefixes`, or `None` for the gateway's default.
    pub wip_prefixes: Option<Vec<String>>,
}

impl Upstream {
    /// Reads `host`'s settings from the shim config and the environment, checking what the
    /// built-in client checks: `FJ_SHIM_CA_FILE` must be absolute, and every CA file must be
    /// readable and hold at least one PEM certificate.
    pub fn for_host(
        host: &str,
        config: &Config,
        env: &HashMap<String, String>,
    ) -> Result<Self, String> {
        let host = config::normalize_host(host);
        let settings = config.host.get(&host);
        let base_text = settings
            .and_then(|settings| settings.api_url.clone())
            .unwrap_or_else(|| format!("https://{host}"));
        let base = Url::parse(&format!("{}/", base_text.trim_end_matches('/')))
            .map_err(|error| format!("invalid base URL for {host}: {error}"))?;

        let mut ca_files: Vec<PathBuf> = settings
            .and_then(|settings| settings.ca_file.clone())
            .into_iter()
            .collect();
        if let Some(path) = env.get("FJ_SHIM_CA_FILE").filter(|path| !path.is_empty()) {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("FJ_SHIM_CA_FILE must be an absolute path".to_string());
            }
            ca_files.push(path);
        }
        for path in &ca_files {
            certificates(path)?;
        }

        Ok(Self {
            base,
            ca_files,
            wip_prefixes: settings.and_then(|settings| settings.wip_prefixes.clone()),
        })
    }

    /// Every extra CA root, parsed.
    pub fn roots(&self) -> Result<Vec<reqwest::Certificate>, String> {
        let mut roots = Vec::new();
        for path in &self.ca_files {
            roots.extend(certificates(path)?);
        }
        Ok(roots)
    }

    /// The gateway's draft prefixes for this host.
    pub fn wip(&self) -> WipPrefixes {
        match &self.wip_prefixes {
            Some(prefixes) => WipPrefixes::from_csv(&prefixes.join(",")),
            None => WipPrefixes::default(),
        }
    }

    /// Whether the gateway's client can honour these settings today: no extra CA roots, and a
    /// base at the forge's root (fga-core joins absolute `/api/v1/...` paths). Until it can,
    /// such a host keeps the built-in path.
    pub fn gateway_ready(&self) -> bool {
        self.ca_files.is_empty() && self.base.path() == "/"
    }
}

/// The PEM certificates in a CA file, as the built-in client reads them.
fn certificates(path: &Path) -> Result<Vec<reqwest::Certificate>, String> {
    let pem = std::fs::read(path)
        .map_err(|error| format!("could not read CA certificate {}: {error}", path.display()))?;
    let certificates = reqwest::Certificate::from_pem_bundle(&pem)
        .map_err(|error| format!("invalid CA certificate {}: {error}", path.display()))?;
    if certificates.is_empty() {
        return Err(format!(
            "invalid CA certificate {}: no PEM certificates found",
            path.display()
        ));
    }
    Ok(certificates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EnvMap;

    fn test_ca() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-ca.pem")
    }

    fn config(toml: &str) -> Config {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gfj-upstream-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, toml).expect("config");
        let config = config::load_config_with_env(Some(&path), &EnvMap::new()).expect("load");
        std::fs::remove_dir_all(&dir).expect("cleanup");
        config
    }

    #[test]
    fn the_base_is_api_url_or_the_forge_itself() {
        let none = HashMap::new();
        let plain = Upstream::for_host("forge.test", &config("hosts = [\"forge.test\"]\n"), &none)
            .expect("plain");
        assert_eq!(plain.base.as_str(), "https://forge.test/");
        assert!(plain.gateway_ready());
        let api = config("[host.\"forge.test\"]\napi_url = \"http://127.0.0.1:3000/forgejo\"\n");
        let upstream = Upstream::for_host("forge.test", &api, &none).expect("api_url");
        assert_eq!(upstream.base.as_str(), "http://127.0.0.1:3000/forgejo/");
        assert!(
            !upstream.gateway_ready(),
            "a path prefix waits for fga-core"
        );
    }

    #[test]
    fn ca_files_come_from_the_host_and_the_environment() {
        let ca = test_ca();
        let with_host_ca = config(&format!(
            "[host.\"forge.test\"]\nca_file = \"{}\"\n",
            ca.display()
        ));
        let env = HashMap::from([("FJ_SHIM_CA_FILE".to_string(), ca.display().to_string())]);
        let upstream = Upstream::for_host("forge.test", &with_host_ca, &env).expect("ca");
        assert_eq!(upstream.ca_files, vec![ca.clone(), ca.clone()]);
        assert!(!upstream.roots().expect("roots").is_empty());
        assert!(!upstream.gateway_ready(), "extra roots wait for fga-core");
    }

    #[test]
    fn bad_ca_files_are_errors_as_on_the_built_in_path() {
        let plain = config("hosts = [\"forge.test\"]\n");
        let relative = HashMap::from([("FJ_SHIM_CA_FILE".to_string(), "ca.pem".to_string())]);
        assert_eq!(
            Upstream::for_host("forge.test", &plain, &relative).expect_err("relative"),
            "FJ_SHIM_CA_FILE must be an absolute path"
        );
        let missing = HashMap::from([(
            "FJ_SHIM_CA_FILE".to_string(),
            "/nonexistent/ca.pem".to_string(),
        )]);
        assert!(Upstream::for_host("forge.test", &plain, &missing)
            .expect_err("missing")
            .starts_with("could not read CA certificate /nonexistent/ca.pem"));
        let not_pem = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let not_pem_env =
            HashMap::from([("FJ_SHIM_CA_FILE".to_string(), not_pem.display().to_string())]);
        assert!(Upstream::for_host("forge.test", &plain, &not_pem_env)
            .expect_err("not pem")
            .contains("no PEM certificates found"));
    }

    #[test]
    fn wip_prefixes_reach_the_gateway_or_keep_its_default() {
        let none = HashMap::new();
        let set = config("[host.\"forge.test\"]\nwip_prefixes = [\"DRAFT:\"]\n");
        let upstream = Upstream::for_host("forge.test", &set, &none).expect("set");
        assert_eq!(upstream.wip_prefixes, Some(vec!["DRAFT:".to_string()]));
        assert_eq!(upstream.wip().strip("DRAFT: x"), "x");
        assert_eq!(
            upstream.wip().strip("WIP: x"),
            "WIP: x",
            "only the forge's own prefixes"
        );
        let unset = Upstream::for_host("forge.test", &config("hosts = [\"forge.test\"]\n"), &none)
            .expect("unset");
        assert_eq!(unset.wip_prefixes, None);
        assert_eq!(unset.wip().strip("WIP: x"), "x", "the gateway's default");
    }
}
