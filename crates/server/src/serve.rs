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
    routing::{get, post},
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
use recto_domain::{
    held::{self, files::HeldFiles},
    ledger,
};
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
    device_auth::{self, Listener},
    dnssd, lan,
    routes::{
        administration, auth, devices, escl, held as held_routes, ipp, password, print, pwa, scan,
    },
    session, state,
};

/// How often expired scan results are deleted.
const PURGE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How often expired sessions are deleted.
const SESSION_PURGE_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// How often expired held jobs are deleted.
const HELD_SWEEP_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How old a held document no row names must be before a sweep removes it while recto runs: one
/// written moments ago may be waiting for its row.
const ORPHAN_AGE: Duration = Duration::from_secs(60 * 60);

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
/// `/offline`, the printer's and scanners' addresses for devices under `/ipp/`, `/eSCL/` and
/// `/device/` (which authenticate devices themselves, see [`ipp`] and [`escl`]), and the static
/// files the fallback serves
/// (see `fallback`). The
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
    let held_jobs = held_routes::router(state.clone(), &auth);
    let administration = administration::router(state.clone(), &auth);
    let devices_ipp = device_routes(&state, &auth, Listener::Main);
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
    let web = Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(auth::router().with_state(auth.clone()))
        .merge(password)
        .merge(devices)
        .merge(held_jobs)
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
        ));
    // Devices have no session and send no `Origin`: they authenticate in their routes. Any
    // other address under theirs is a bare 404, never the sign-in redirect or the not-found
    // page of the fallback, which would carry a secret into a `Location` header or the
    // browser's history.
    Router::new()
        .merge(devices_ipp)
        .merge(web)
        .layer(middleware::from_fn(security_headers))
}

/// The routes of the LAN listener: the device addresses only, never a page or a session.
pub fn lan_router(state: AppState, auth: AuthState) -> Router {
    device_routes(&state, &auth, Listener::Lan).layer(middleware::from_fn(security_headers))
}

/// The printer's and the scanners' addresses, and the bare 404 of any other device address.
fn device_routes(state: &AppState, auth: &AuthState, listener: Listener) -> Router {
    escl::routes(
        ipp::router(state.clone(), auth, listener),
        state.clone(),
        auth,
        listener,
    )
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
    // Before serving, while nothing is being written: a crash between writing a document and
    // recording it left a file no row names.
    remove_orphan_held_files(&state.database, &state.held, SystemTime::now()).await;
    start_maintenance(&state, &auth);
    // Before serving: a certificate, an address or an interface that is wrong stops recto here.
    let lan = match config.lan_listen_address {
        Some(lan_address) => Some(prepare_lan(&config, lan_address, &state.branding.name).await?),
        None => None,
    };
    let (stop, stopped) = tokio::sync::watch::channel(());
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = stop.send(());
    });
    let mut lan_server = None;
    let mut announcing = None;
    if let Some(lan) = lan {
        if let Some(dnssd) = lan.dnssd {
            announcing = Some(announce(&state, dnssd, stopped.clone()));
        }
        let routes = lan_router(state.clone(), auth.clone());
        lan_server = Some(tokio::spawn(lan::serve(
            lan.listener,
            routes,
            until(stopped.clone()),
        )));
    }
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
    // The peer address is what the sign-in rate limit counts attempts by. The header timeout
    // matters even behind a proxy: recto may be reachable without one on the host's network.
    crate::connections::serve(
        listener,
        app,
        crate::connections::HEADER_READ_TIMEOUT,
        until(stopped),
    )
    .await?;
    if let Some(lan_server) = lan_server {
        lan_server.await??;
    }
    // The goodbyes go out once nothing is served any more.
    if let Some(announcing) = announcing {
        let _ = announcing.await;
    }
    Ok(())
}

/// Resolves once the server is told to stop.
async fn until(mut stopped: tokio::sync::watch::Receiver<()>) {
    // An error means the sender is gone, which also means stopping.
    let _ = stopped.changed().await;
}

/// The LAN listener, bound and with its certificate, and what DNS-SD announces on it.
struct Lan {
    listener: lan::TlsListener,
    dnssd: Option<Announcement>,
}

/// The printer to announce, and the responder that will, already listening.
struct Announcement {
    printer: dnssd::Advertised,
    registry: dnssd::MdnsRegistry,
}

/// Loads or creates the LAN certificate, binds the LAN listener and, with `RECTO_DNSSD`, starts
/// the mDNS responder on the chosen interfaces.
async fn prepare_lan(
    config: &Config,
    address: SocketAddr,
    brand_name: &str,
) -> Result<Lan, Box<dyn std::error::Error>> {
    let hostname = dnssd::system_hostname()?;
    let certificate = config.lan_tls_certificate.clone();
    let key = config.lan_tls_key.clone();
    let data_dir = config.data_dir.clone();
    let names = hostname.clone();
    let tls = tokio::task::spawn_blocking(move || {
        lan::load_or_create(
            certificate.as_deref(),
            key.as_deref(),
            &data_dir,
            &names,
            &[address.ip()],
        )
    })
    .await??;
    let tcp = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("RECTO_LAN_LISTEN_ADDRESS {address}: {error}"))?;
    // People compare it with what their device shows the first time it connects.
    tracing::info!(%address, fingerprint = %tls.fingerprint, "LAN listener");
    let listener = lan::TlsListener::new(tcp, tls.acceptor(), lan::HANDSHAKE_TIMEOUT);
    if !config.dnssd {
        return Ok(Lan {
            listener,
            dnssd: None,
        });
    }
    let interfaces = dnssd::interfaces(&config.dnssd_interfaces, &dnssd::candidates()?)?;
    if interfaces.is_empty() {
        tracing::warn!(
            "RECTO_DNSSD found no interface to announce on: set RECTO_DNSSD_INTERFACES, or run \
             recto on the host's network"
        );
    }
    // A listener bound to an IPv4 address would leave IPv6 clients at a closed port; `[::]` is
    // dual stack on Linux.
    let ipv6 = address.is_ipv6();
    let started = interfaces.clone();
    let registry =
        tokio::task::spawn_blocking(move || dnssd::MdnsRegistry::start(&started, ipv6)).await??;
    let instance = config.dnssd_name.as_deref().unwrap_or(brand_name);
    let host = dnssd::advertised_host(&hostname);
    tracing::info!(host, interfaces = interfaces.join(","), "DNS-SD responder");
    Ok(Lan {
        listener,
        dnssd: Some(Announcement {
            printer: dnssd::printer(instance, &host, address.port(), &config.public_url),
            registry,
        }),
    })
}

/// Announces the printer and the scanners open to everyone until the server stops, then
/// withdraws them. The scanner list may take seconds, so this runs in the background.
fn announce(
    state: &AppState,
    announcement: Announcement,
    stopped: tokio::sync::watch::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let scan = Arc::clone(&state.scan);
    let database = state.database.clone();
    tokio::spawn(async move {
        let announced = dnssd::announce(
            announcement.printer,
            &*scan,
            &database,
            announcement.registry,
        )
        .await;
        let responder = match announced {
            Ok(responder) => responder,
            Err(error) => {
                tracing::error!(%error, "announcing recto over DNS-SD failed");
                return;
            }
        };
        tracing::info!(
            services = responder.fullnames().join(", "),
            "announced over DNS-SD"
        );
        until(stopped).await;
        // Unregistering waits for the goodbye packets.
        let withdrawn = tokio::task::spawn_blocking(move || responder.shutdown()).await;
        if let Err(error) = withdrawn {
            tracing::warn!(%error, "withdrawing the DNS-SD services failed");
        }
    })
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

/// Starts the background tasks: the held jobs' sweep, the purge of scan results and sessions,
/// the ledger's job polling and the first scanner listing. None depends on OS integration:
/// documents held before it was turned off must still go.
pub fn start_maintenance(state: &AppState, auth: &AuthState) {
    sweep_held(state.database.clone(), Arc::clone(&state.held));
    purge_expired_results(Arc::clone(&state.results));
    delete_expired_sessions(auth.sessions.clone());
    poll_jobs(Arc::clone(&state.print), state.database.clone());
    find_scanners(Arc::clone(&state.scan));
}

/// Sweeps the held jobs in the background, every [`HELD_SWEEP_INTERVAL`].
fn sweep_held(database: DatabaseConnection, files: Arc<HeldFiles>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(HELD_SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            sweep_held_once(&database, &files, chrono::Utc::now()).await;
        }
    });
}

/// One sweep of the held jobs: deletes the expired ones and their documents, returns the
/// releases that never reached the ledger (see [`held::sweep`]), and removes documents no row
/// has named for an hour. Failures are logged and left to the next sweep.
pub async fn sweep_held_once(
    database: &DatabaseConnection,
    files: &Arc<HeldFiles>,
    now: chrono::DateTime<chrono::Utc>,
) {
    match held::sweep(database, now).await {
        Ok(swept) => {
            if swept.expired > 0 {
                tracing::info!(count = swept.expired, "held jobs expired");
            }
            if swept.abandoned > 0 {
                tracing::info!(
                    count = swept.abandoned,
                    "held jobs whose document never came were removed"
                );
            }
            if swept.returned > 0 {
                tracing::warn!(
                    count = swept.returned,
                    "held jobs whose release never finished are held again"
                );
            }
            remove_held_files(files, swept.files).await;
        }
        Err(error) => tracing::error!(%error, "sweeping the held jobs failed"),
    }
    return_unsent_held(database, files, now).await;
    let written_before = SystemTime::now()
        .checked_sub(ORPHAN_AGE)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    remove_orphan_held_files(database, files, written_before).await;
}

/// Puts back to `held` the released jobs that never reached CUPS and still have their document:
/// CUPS refused them and returning them failed, or recto stopped before sending them. Their
/// ledger entry is refunded with them.
async fn return_unsent_held(
    database: &DatabaseConnection,
    files: &Arc<HeldFiles>,
    now: chrono::DateTime<chrono::Utc>,
) {
    let unsent = match held::unsent(database, now).await {
        Ok(unsent) => unsent,
        Err(error) => {
            tracing::error!(%error, "reading the unsent held jobs failed");
            return;
        }
    };
    for job in unsent {
        let (Some(file), Some(entry)) = (job.file.clone(), job.ledger_entry_id) else {
            continue;
        };
        let held_files = Arc::clone(files);
        let present = tokio::task::spawn_blocking(move || held_files.dir().join(&file).is_file())
            .await
            .unwrap_or(false);
        if !present {
            continue;
        }
        let code = recto_app::refusal::Refusal::Unavailable.code();
        match held::return_refunded(database, job.id, job.claim, entry, code, now).await {
            Ok(()) => tracing::warn!(
                job = job.id,
                entry,
                "a held job that never reached the printing service is held again"
            ),
            Err(error) => {
                tracing::error!(%error, job = job.id, "returning an unsent held job failed")
            }
        }
    }
}

/// Removes the held documents no row names, written before `written_before`, and returns how
/// many there were.
pub async fn remove_orphan_held_files(
    database: &DatabaseConnection,
    files: &Arc<HeldFiles>,
    written_before: SystemTime,
) -> usize {
    let known = match held::files(database).await {
        Ok(known) => known,
        Err(error) => {
            tracing::error!(%error, "reading the held documents' names failed");
            return 0;
        }
    };
    let listing = Arc::clone(files);
    let orphans =
        tokio::task::spawn_blocking(move || listing.orphans(&known, written_before)).await;
    let orphans = match orphans {
        Ok(Ok(orphans)) => orphans,
        Ok(Err(error)) => {
            tracing::warn!(%error, dir = %files.dir().display(), "listing the held documents failed");
            return 0;
        }
        Err(error) => {
            tracing::warn!(%error, "listing the held documents failed");
            return 0;
        }
    };
    let count = orphans.len();
    if count > 0 {
        tracing::info!(count, "orphan held files removed");
        remove_held_files(files, orphans).await;
    }
    count
}

/// Removes held documents; one that cannot be removed is logged, and the orphan sweep tries
/// again.
async fn remove_held_files(files: &Arc<HeldFiles>, names: Vec<String>) {
    if names.is_empty() {
        return;
    }
    let files = Arc::clone(files);
    let removed = tokio::task::spawn_blocking(move || {
        for name in names {
            if let Err(error) = files.remove(&name) {
                tracing::warn!(%error, file = name, "removing a held document failed");
            }
        }
    })
    .await;
    if let Err(error) = removed {
        tracing::warn!(%error, "removing held documents failed");
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
