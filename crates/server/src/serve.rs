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
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use leptos::config::LeptosOptions;
use leptos::prelude::provide_context;
use leptos_axum::{LeptosRoutes, generate_route_list_with_exclusions};
use recto_app::{
    App,
    branding::Branding,
    pages::sign_in::SignInMethods,
    shell,
    state::{AppState, User},
};
use recto_authentication::csrf::{self, AllowedOrigin};
use recto_domain::ledger;
use recto_printing::PrintService;
use recto_scanning::{ResultStore, ScannerService};
use sea_orm::DatabaseConnection;
use tower::ServiceExt;
use tower_http::services::ServeDir;
use tower_sessions::session_store::ExpiredDeletion;

use crate::session_store::DatabaseSessionStore;

pub use crate::state::AuthState;
use crate::{
    branding,
    config::Config,
    device_auth,
    routes::{administration, auth, devices, password, print, pwa, scan},
    session, state,
};

/// How often expired scan results are deleted.
const PURGE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How often expired sessions are deleted.
const SESSION_PURGE_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// How often CUPS is asked about the jobs of the ledger that are not final.
const JOB_POLL_INTERVAL: Duration = Duration::from_secs(30);

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
/// while they are down), the sign-in addresses (`/sign-in`, the `/auth/*` routes and pages,
/// `/no-access`), `POST /theme`, `/brand/*`, `/manifest.webmanifest`, `/sw.js`, `/pwa.js`,
/// `/offline`, the personal device addresses `/device/*` (a bare 404 for now), and the static
/// files the fallback serves (see `fallback`). The
/// `/administration` pages and forms need an administrator.
///
/// Must be called inside a Tokio runtime: the sign-in rate limit forgets old clients in the
/// background.
pub fn router(state: AppState, auth: AuthState) -> Router {
    // Pages rendered by their own routes: the sign-in page first sends signed-in users on and,
    // with only single sign-on, visitors to the provider; the administration pages load their
    // data only for administrators.
    let excluded = [
        "/sign-in",
        "/administration/users",
        "/administration/groups",
        "/administration/printers",
        "/administration/scanners",
        "/administration/quotas",
        "/administration/usage",
    ];
    let routes = generate_route_list_with_exclusions(
        App,
        Some(excluded.into_iter().map(str::to_owned).collect()),
    );
    let shell_options = state.leptos_options.clone();
    let branding = Arc::clone(&state.branding);
    let branding_for_refusal = Arc::clone(&branding);
    let body_limit = state.max_upload_size.saturating_add(print::FORM_SLACK);
    let methods = auth.sign_in_methods();
    let context = move || provide_context(methods);
    let password = password::router(state.clone(), &auth);
    let devices = devices::router(state.clone(), &auth);
    let administration = administration::router(state.clone(), &auth);
    let interface = Router::new()
        .route(
            "/print",
            post(print::submit).layer(DefaultBodyLimit::max(body_limit)),
        )
        .route("/jobs/{id}/cancel", post(print::cancel))
        .route("/scan/results/{id}", get(scan::show))
        .route("/scan/results/{id}/download", get(scan::download))
        .route("/scan/results/{id}/save", post(scan::save))
        .leptos_routes_with_context(&state, routes, context, move || {
            shell(shell_options.clone())
        })
        // Only the routes above: the fallback checks the user itself, after serving assets.
        .route_layer(middleware::from_fn(session::require_user))
        .fallback(move |state, request| fallback(state, methods, request))
        .with_state(state);
    Router::new()
        .route("/health", get(|| async { "ok" }))
        // Until a device route matches, a personal address is unknown: a bare 404, never the
        // sign-in redirect or the not-found page of the fallback, which would carry the secret
        // into a `Location` header or the browser's history.
        .route("/device/{*rest}", any(|| async { StatusCode::NOT_FOUND }))
        .merge(auth::router().with_state(auth.clone()))
        .merge(password)
        .merge(devices)
        .merge(pwa::router(branding, auth.secure_cookies))
        .merge(administration)
        .merge(interface)
        .layer(middleware::from_fn_with_state(
            auth.database.clone(),
            session::load_user,
        ))
        .layer(session::layer(auth.sessions.clone(), auth.secure_cookies))
        .layer(middleware::from_fn_with_state(
            (AllowedOrigin::new(&auth.public_url), branding_for_refusal),
            refuse_cross_site,
        ))
        .layer(middleware::from_fn(security_headers))
}

/// Headers for every response. Pages bring their own Content Security Policy, with the nonce
/// of their hydration script (see [`recto_app::content_security_policy`]); everything else
/// gets one that only forbids framing, so no response can be put in another site's frame.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("frame-ancestors 'none'"),
        );
    }
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Rejects cross-site state-changing requests with 403 (see [`csrf::allowed`]).
async fn refuse_cross_site(
    State((origin, branding)): State<(AllowedOrigin, Arc<Branding>)>,
    request: Request,
    next: Next,
) -> Response {
    if csrf::allowed(request.method(), request.headers(), &origin) {
        return next.run(request).await;
    }
    tracing::warn!(
        method = %request.method(),
        path = %device_auth::loggable_path(request.uri().path()),
        "cross-site request refused"
    );
    let locale = recto_app::i18n::from_headers(request.headers());
    let message = recto_app::messages::cross_site_refused(locale, &branding.name, origin.as_str());
    (StatusCode::FORBIDDEN, message).into_response()
}

/// Loads the branding, opens and migrates the database, then serves until ctrl-c or SIGTERM;
/// requests in flight finish. Branding that is not valid or a database that cannot be opened
/// stops recto before anything else starts.
pub async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let address = config.listen_address;
    let branding = branding::load(&config.brand)?;
    let database = state::open_database(&config.database_url).await?;
    #[cfg_attr(
        not(all(feature = "dev-mocks", debug_assertions)),
        expect(unused_mut, reason = "only the development mocks change the state")
    )]
    let (mut state, auth) =
        state::build(&config, leptos_options(&config), database, branding).await;
    #[cfg(all(feature = "dev-mocks", debug_assertions))]
    let dev_user = if crate::dev::requested(std::env::var("RECTO_DEV_MOCKS").ok().as_deref()) {
        let user = crate::dev::apply(&mut state).await?;
        tracing::warn!(
            "RECTO_DEV_MOCKS=1: printers and scanners are MOCKS and EVERY REQUEST IS SIGNED IN \
             as the development user. Never expose this server."
        );
        Some(user)
    } else {
        None
    };
    purge_expired_results(Arc::clone(&state.results));
    delete_expired_sessions(auth.sessions.clone());
    poll_jobs(Arc::clone(&state.print), state.database.clone());
    find_scanners(Arc::clone(&state.scan));
    #[cfg_attr(
        not(all(feature = "dev-mocks", debug_assertions)),
        expect(unused_mut, reason = "only the development mocks change the router")
    )]
    let mut app = router(state, auth);
    #[cfg(all(feature = "dev-mocks", debug_assertions))]
    if let Some(user) = dev_user {
        app = app.layer(middleware::from_fn_with_state(user, crate::dev::sign_in));
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "listening");
    // The peer address is what the sign-in rate limit counts attempts by.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// What no route matches: a file of the site root (scripts, styles, icons) for anyone, else
/// the not-found page for a signed-in user. The interface's router is lenient (`/jobs/` renders
/// `/jobs`), so rendering it here for an anonymous visitor would bypass the routes' user check:
/// anonymous visitors get the sign-in redirect or 401 instead.
async fn fallback(
    State(state): State<AppState>,
    methods: SignInMethods,
    request: Request,
) -> Response {
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
    let not_found = leptos_axum::file_and_error_handler_with_context::<AppState, _>(
        move || provide_context(methods),
        shell,
    );
    not_found(request.uri().clone(), State(state), request).await
}

/// Lists the scanners once in the background, so the first visit to the scan page finds the
/// list cached instead of waiting for SANE's discovery, which takes seconds with network
/// backends. Startup does not wait for it.
fn find_scanners(scan: Arc<dyn ScannerService>) {
    tokio::spawn(async move {
        match scan.scanners(true).await {
            Ok(scanners) => tracing::info!(count = scanners.len(), "scanners listed"),
            Err(error) => tracing::warn!(%error, "listing the scanners failed"),
        }
    });
}

/// Records the final state of the ledger's jobs in the background, every [`JOB_POLL_INTERVAL`].
fn poll_jobs(print: Arc<dyn PrintService>, database: DatabaseConnection) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(JOB_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            poll_once(&*print, &database).await;
        }
    });
}

/// One round of the job poller: refunds the entries CUPS never received, then asks CUPS about
/// each entry that is not final. CUPS is only contacted while there is one. Failures are logged
/// and the entries tried again by the next round.
pub async fn poll_once(print: &dyn PrintService, database: &DatabaseConnection) {
    let now = chrono::Utc::now();
    match ledger::refund_unattached(database, now).await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(count, "refunded ledger entries CUPS never received"),
        Err(error) => tracing::error!(%error, "refunding the unsent ledger entries failed"),
    }
    let unfinished = match ledger::unfinished(database).await {
        Ok(unfinished) => unfinished,
        Err(error) => {
            tracing::error!(%error, "reading the unfinished ledger entries failed");
            return;
        }
    };
    for (id, cups_job_id) in unfinished {
        let status = match print.job_status(cups_job_id).await {
            Ok(status) => status,
            Err(error) => {
                // With CUPS down every entry would wait for its own timeout.
                tracing::warn!(%error, job = cups_job_id, "reading a job state failed");
                return;
            }
        };
        if let Err(error) = ledger::observe(database, id, status.as_ref(), now).await {
            tracing::error!(%error, job = cups_job_id, "recording a job state failed");
        }
    }
}

/// Deletes expired sessions in the background, every [`SESSION_PURGE_INTERVAL`].
fn delete_expired_sessions(sessions: DatabaseSessionStore) {
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
