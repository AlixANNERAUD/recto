//! The HTTP routes and the server loop.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{Method, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use leptos::config::LeptosOptions;
use leptos_axum::{LeptosRoutes, generate_route_list};
use recto_app::{App, shell, state::AppState};
use recto_authentication::User;
use recto_authentication::csrf::{self, AllowedOrigin};
use recto_scanning::ResultStore;
use tower::ServiceExt;
use tower_http::services::ServeDir;
use tower_sessions::session_store::ExpiredDeletion;

pub use crate::state::AuthState;
use crate::{
    config::Config,
    routes::{auth, print, scan},
    session, state,
};

/// How often expired scan results are deleted.
const PURGE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How often expired sessions are deleted.
const SESSION_PURGE_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Where the interface's assets are and how they are named.
pub fn leptos_options(config: &Config) -> LeptosOptions {
    LeptosOptions::builder()
        .output_name("recto")
        .site_root(config.site_root.to_string_lossy().into_owned())
        .site_pkg_dir("pkg")
        .site_addr(config.listen_address)
        .build()
}

/// Every route of the server.
///
/// Pages, forms, scan results and server functions need a signed-in user. Left open are
/// `/health` (it touches neither CUPS nor the identity provider, so a container stays healthy
/// while they are down), the `/auth/*` routes and pages, `/no-access`, and the assets the
/// fallback serves (see `fallback`).
pub fn router(state: AppState, auth: AuthState) -> Router {
    let routes = generate_route_list(App);
    let shell_options = state.leptos_options.clone();
    let body_limit = state.max_upload_size.saturating_add(print::FORM_SLACK);
    let interface = Router::new()
        .route(
            "/print",
            post(print::submit).layer(DefaultBodyLimit::max(body_limit)),
        )
        .route("/jobs/{id}/cancel", post(print::cancel))
        .route("/scan/results/{id}", get(scan::show))
        .route("/scan/results/{id}/download", get(scan::download))
        .route("/scan/results/{id}/save", post(scan::save))
        .leptos_routes(&state, routes, move || shell(shell_options.clone()))
        // Only the routes above: the fallback checks the user itself, after serving assets.
        .route_layer(middleware::from_fn(session::require_user))
        .fallback(fallback)
        .with_state(state);
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(auth::router().with_state(auth.clone()))
        .merge(interface)
        .layer(middleware::from_fn(session::load_user))
        .layer(session::layer(auth.sessions.clone(), auth.secure_cookies))
        .layer(middleware::from_fn_with_state(
            AllowedOrigin::new(&auth.public_url),
            csrf::protect,
        ))
}

/// Serves until ctrl-c or SIGTERM; requests in flight finish.
pub async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let address = config.listen_address;
    let (state, auth) = state::build(&config, leptos_options(&config)).await;
    purge_expired_results(Arc::clone(&state.results));
    delete_expired_sessions(auth.sessions.clone());
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "listening");
    axum::serve(listener, router(state, auth))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// What no route matches: a file of the site root (scripts, styles, icons) for anyone, else
/// the not-found page for a signed-in user. The interface's router is lenient (`/jobs/` renders
/// `/jobs`), so rendering it here for an anonymous visitor would bypass the routes' user check:
/// anonymous visitors get the sign-in redirect or 401 instead.
async fn fallback(State(state): State<AppState>, request: Request) -> Response {
    if matches!(*request.method(), Method::GET | Method::HEAD) {
        let mut probe = Request::new(Body::empty());
        *probe.method_mut() = request.method().clone();
        *probe.uri_mut() = request.uri().clone();
        *probe.headers_mut() = request.headers().clone();
        let files = ServeDir::new(&*state.leptos_options.site_root)
            .precompressed_gzip()
            .precompressed_br();
        let Ok(response) = files.oneshot(probe).await;
        if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
            return response.into_response();
        }
    }
    if request.extensions().get::<User>().is_none() {
        return session::unauthenticated(&request);
    }
    let not_found = leptos_axum::file_and_error_handler::<AppState, _>(shell);
    not_found(request.uri().clone(), State(state), request).await
}

/// Deletes expired sessions in the background, every [`SESSION_PURGE_INTERVAL`].
fn delete_expired_sessions(sessions: session::Store) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SESSION_PURGE_INTERVAL);
        loop {
            interval.tick().await;
            if let Err(error) = sessions.delete_expired().await {
                tracing::warn!(%error, "deleting expired sessions failed");
            }
        }
    });
}

/// Deletes expired scan results in the background, every [`PURGE_INTERVAL`].
fn purge_expired_results(results: Arc<ResultStore>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(PURGE_INTERVAL);
        loop {
            interval.tick().await;
            let results = Arc::clone(&results);
            // Deleting files blocks.
            let purged =
                tokio::task::spawn_blocking(move || results.purge_expired(SystemTime::now())).await;
            if let Err(error) = purged {
                tracing::warn!(%error, "purging expired scan results failed");
            }
        }
    });
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
