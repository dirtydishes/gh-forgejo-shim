use fga_core::forgejo::Forgejo;
use fga_server::{AppState, Host, app, config::Config};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    // Server mode serves one forge and lets Forgejo judge every user's token.
    let host = Host::new(
        config.public_url,
        Forgejo::new(config.forgejo_url)?,
        config.wip_prefixes,
    );
    let state = AppState::new(vec![host], config.ghes_version);
    let router = app(state);
    if let Some((cert, key)) = config.tls {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("one TLS provider");
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
        axum_server::bind_rustls(config.listen, tls)
            .serve(router.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(config.listen).await?;
        axum::serve(listener, router).await?;
    }
    Ok(())
}
