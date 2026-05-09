mod auth;
mod cli;
mod tls;
mod upload;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use axum::Router;
use clap::Parser;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// 5 GiB upload chunk ceiling. Dropzone defaults to ~2 MiB chunks; the
/// generous limit lets us tolerate chunky single-chunk uploads.
const REQUEST_BODY_LIMIT: usize = 5 * 1024 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = cli::Cli::parse();

    if !args.upload_dir.exists() {
        std::fs::create_dir_all(&args.upload_dir)
            .with_context(|| format!("creating upload dir {}", args.upload_dir.display()))?;
    }

    info!(
        ssl = args.ssl,
        port = args.port,
        upload_dir = %args.upload_dir.display(),
        "starting flowcase_upload_server"
    );

    let upload_dir = upload::UploadDir::new(args.upload_dir.clone());
    let auth_token = Arc::new(args.auth_token.clone());

    let app = Router::new()
        .route("/upload", post(upload::handle_upload))
        .with_state(upload_dir)
        .layer(from_fn_with_state(auth_token, auth::require_basic_auth))
        .layer(RequestBodyLimitLayer::new(REQUEST_BODY_LIMIT));

    let addr = SocketAddr::from(([0, 0, 0, 0], args.port));

    if args.ssl {
        let config = tls::build_self_signed_config().context("building self-signed TLS config")?;
        let rustls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(config));
        info!(%addr, "listening with self-signed TLS");
        tokio::select! {
            result = axum_server::bind_rustls(addr, rustls).serve(app.into_make_service()) => {
                result.context("https server")?;
            }
            _ = tokio::signal::ctrl_c() => info!("ctrl_c received, shutting down"),
        }
    } else {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(%addr, "listening (plain http)");
        tokio::select! {
            result = axum::serve(listener, app) => {
                result.context("http server")?;
            }
            _ = tokio::signal::ctrl_c() => info!("ctrl_c received, shutting down"),
        }
    }

    Ok(())
}
