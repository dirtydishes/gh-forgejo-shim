use fga_core::map::WipPrefixes;
use std::{env, net::SocketAddr, path::PathBuf};
use url::Url;

pub struct Config {
    pub forgejo_url: Url,
    pub public_url: Url,
    pub listen: SocketAddr,
    pub ghes_version: String,
    pub wip_prefixes: WipPrefixes,
    pub tls: Option<(PathBuf, PathBuf)>,
}

impl Config {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let tls = match (env::var_os("FGA_TLS_CERT"), env::var_os("FGA_TLS_KEY")) {
            (Some(cert), Some(key)) => Some((cert.into(), key.into())),
            (None, None) => None,
            _ => return Err("FGA_TLS_CERT and FGA_TLS_KEY must be set together".into()),
        };
        Ok(Self {
            forgejo_url: base_url(&env::var("FGA_FORGEJO_URL")?)?,
            public_url: base_url(&env::var("FGA_PUBLIC_URL")?)?,
            listen: env::var("FGA_LISTEN")
                .unwrap_or_else(|_| "127.0.0.1:8080".into())
                .parse()?,
            ghes_version: env::var("FGA_GHES_VERSION").unwrap_or_else(|_| "3.19.0".into()),
            wip_prefixes: env::var("FGA_WIP_PREFIXES")
                .map(|value| WipPrefixes::from_csv(&value))
                .unwrap_or_default(),
            tls,
        })
    }
}

fn base_url(value: &str) -> Result<Url, Box<dyn std::error::Error>> {
    let url = Url::parse(value)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "FGA URLs must be HTTP(S) base URLs without credentials, query or fragment".into(),
        );
    }
    Ok(url)
}
