//! The gateway inside a launched call (launcher spec §5): the fga-server router, one `Host` per
//! served host, behind a unix listener that only the launcher's own uid can use.

use std::io;
use std::time::Duration;

use axum::Router;
use fga_core::forgejo::Forgejo;
use fga_core::map::WipPrefixes;
use fga_server::{AppState, Host};
use tokio::net::unix::SocketAddr;
use tokio::net::{UnixListener, UnixStream};
use url::Url;

use crate::repo::gh_hostname;

/// The GitHub Enterprise version the gateway reports, as fga-server's own default.
pub const GHES_VERSION: &str = "3.19.0";

/// One served host's gateway settings.
#[derive(Debug, Clone)]
pub struct GatewayHost {
    /// The allowlist entry: `forge.test` or `forge.test:3443`.
    pub entry: String,
    /// The forge's own base URL, where Forgejo's API lives.
    pub forgejo_url: Url,
    /// The token gh presents: the per-call one, or the real one in a `--show-token` call.
    pub expected: String,
    /// The token the gateway sends upstream.
    pub upstream: String,
    /// Whether a ported entry also answers to its portless hostname. False when another served
    /// entry shares the hostname (§3 keeps those out of *S*, so this is a safeguard).
    pub answer_portless: bool,
    pub wip_prefixes: WipPrefixes,
}

/// Builds the router for every served host.
pub fn router(hosts: Vec<GatewayHost>) -> Result<Router, String> {
    let mut built = Vec::with_capacity(hosts.len());
    for host in hosts {
        let public_url = Url::parse(&format!("https://{}", host.entry))
            .map_err(|error| format!("{}: {error}", host.entry))?;
        let forgejo = Forgejo::new(host.forgejo_url.clone())
            .map_err(|error| format!("{}: {error}", host.entry))?;
        let mut gateway_host = Host::new(public_url, forgejo, host.wip_prefixes.clone())
            .expecting(host.expected.clone())
            .upstream_token(host.upstream.clone());
        let hostname = gh_hostname(&host.entry);
        if host.answer_portless && hostname != host.entry {
            gateway_host = gateway_host
                .answering_to(&hostname)
                .map_err(|error| format!("{}: {error}", host.entry))?;
        }
        built.push(gateway_host);
    }
    let state = AppState::new(built, GHES_VERSION.to_string()).map_err(str::to_string)?;
    Ok(fga_server::app(state))
}

/// A unix listener that drops any connection whose peer runs as another uid (§4). The 0700 run
/// directory is the real access control; this is a cheap second check.
pub struct PeerChecked {
    listener: UnixListener,
    uid: u32,
}

impl PeerChecked {
    pub fn new(listener: UnixListener) -> Self {
        Self {
            listener,
            uid: rustix::process::getuid().as_raw(),
        }
    }
}

impl axum::serve::Listener for PeerChecked {
    type Io = UnixStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, addr)) => {
                    if stream.peer_cred().is_ok_and(|cred| cred.uid() == self.uid) {
                        return (stream, addr);
                    }
                }
                // As axum's own listeners do: a failed accept (EMFILE, a reset connection)
                // mustn't end the loop, and a short pause keeps it from spinning.
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(entry: &str, answer_portless: bool) -> GatewayHost {
        GatewayHost {
            entry: entry.to_string(),
            forgejo_url: Url::parse(&format!("https://{entry}/")).expect("url"),
            expected: "per-call".to_string(),
            upstream: "real".to_string(),
            answer_portless,
            wip_prefixes: WipPrefixes::default(),
        }
    }

    #[test]
    fn builds_for_default_and_ported_entries() {
        assert!(router(vec![host("forge.test", true), host("port.test:3443", true)]).is_ok());
    }

    #[test]
    fn refuses_an_empty_host_list() {
        assert!(router(Vec::new()).is_err());
    }

    #[test]
    fn two_hosts_answering_to_one_name_are_refused() {
        // §3 keeps such entries out of S; the gateway refuses them anyway (L1).
        assert!(router(vec![
            host("forge.test", true),
            host("forge.test:3443", true)
        ])
        .is_err());
        assert!(router(vec![
            host("forge.test", true),
            host("forge.test:3443", false)
        ])
        .is_ok());
    }
}
