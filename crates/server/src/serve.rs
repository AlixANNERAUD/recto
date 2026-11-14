//! The HTTP routes and the server loop.

use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use axum::{Router, routing::get};
use leptos::config::LeptosOptions;
use leptos_axum::{LeptosRoutes, generate_route_list};
use recto_app::{App, shell};

use crate::config::Config;

/// Where the interface's assets are and how they are named.
pub fn leptos_options(config: &Config) -> LeptosOptions {
    LeptosOptions::builder()
        .output_name("recto")
        .site_root(config.site_root.to_string_lossy().into_owned())
        .site_pkg_dir("pkg")
        .site_addr(config.listen_address)
        .build()
}

/// Every route of the server. `/health` answers without touching CUPS or the identity provider,
/// so a container stays healthy while they are down.
pub fn router(options: LeptosOptions) -> Router {
    let routes = generate_route_list(App);
    let shell_options = options.clone();
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .leptos_routes(&options, routes, move || shell(shell_options.clone()))
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(options)
}

/// Serves until ctrl-c or SIGTERM; requests in flight finish.
pub async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let address = config.listen_address;
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "listening");
    axum::serve(listener, router(leptos_options(&config)))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Succeeds when the server listening on `address` answers `/health` with 200.
pub async fn healthcheck(address: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    // A server bound to every interface is reached over loopback.
    let ip = if address.ip().is_unspecified() {
        match address.ip() {
            IpAddr::V4(_) => IpAddr::from([127, 0, 0, 1]),
            IpAddr::V6(_) => IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
        }
    } else {
        address.ip()
    };
    let url = format!("http://{}/health", SocketAddr::new(ip, address.port()));
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(&url)
        .send()
        .await?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("{url} answered {}", response.status()).into())
    }
}

async fn shutdown_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c().await.ok();
    };
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn health_answers_ok() {
        let options = LeptosOptions::builder().output_name("recto").build();
        let response = router(options)
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"ok");
    }
}
