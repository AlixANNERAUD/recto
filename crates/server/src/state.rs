//! Building the services from the configuration.

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use recto_app::{branding::Branding, pages::sign_in::SignInMethods, state::AppState};
use recto_authentication::{OpenId, OpenIdSettings};
use recto_domain::passwords::{self, PasswordPolicy};
use recto_printing::Cups;
use recto_scanning::{
    ResultStore, SaneScanners, ScanError, ScanRequest, ScannedPage, ScannerCapabilities,
    ScannerInfo, ScannerService,
};
use sea_orm::DatabaseConnection;
use url::Url;

use crate::{config::Config, session_store::DatabaseSessionStore};

/// How long a finished scan waits to be downloaded or saved.
pub const SCAN_RESULT_TTL: Duration = Duration::from_secs(60 * 60);

/// What the sign-in routes need.
#[derive(Clone)]
pub struct AuthState {
    /// The OpenID Connect provider; `None` when only local accounts sign in.
    pub openid: Option<Arc<OpenId>>,
    /// The address users reach recto at, which CSRF checks compare `Origin` against.
    pub public_url: Url,
    /// Whether the session cookie is `Secure`: browsers drop such cookies over plain HTTP.
    pub secure_cookies: bool,
    /// The accounts database, where sign-in finds or creates users.
    pub database: DatabaseConnection,
    /// The sessions, in the same database; the server deletes expired ones in the background.
    pub sessions: DatabaseSessionStore,
    /// Whether local accounts sign in with a password (`/sign-in`).
    pub local_accounts: bool,
    /// The rules of new passwords.
    pub password_policy: PasswordPolicy,
    /// Whether the client address of a password sign-in, which its rate limit counts attempts
    /// by, comes from the forwarded headers rather than from the connection.
    pub trust_forwarded_headers: bool,
}

impl AuthState {
    /// The sign-in state with the defaults of the configuration: local accounts on, passwords of
    /// at least [`passwords::DEFAULT_MINIMUM_LENGTH`] characters, forwarded headers ignored.
    pub fn new(openid: Option<Arc<OpenId>>, public_url: Url, database: DatabaseConnection) -> Self {
        let secure_cookies = public_url.scheme() == "https";
        Self {
            openid,
            public_url,
            secure_cookies,
            sessions: DatabaseSessionStore::new(database.clone()),
            database,
            local_accounts: true,
            password_policy: PasswordPolicy {
                minimum_length: passwords::DEFAULT_MINIMUM_LENGTH,
            },
            trust_forwarded_headers: false,
        }
    }

    /// How people sign in, for the pages.
    pub fn sign_in_methods(&self) -> SignInMethods {
        SignInMethods {
            password: self.local_accounts,
            single_sign_on: self.openid.is_some(),
            password_minimum_length: self.password_policy.minimum_length,
        }
    }

    /// An address under the public URL.
    pub fn public(&self, path: &str) -> Url {
        public_path(&self.public_url, path)
    }
}

/// `path` under `public_url`, whether or not the public URL ends with a slash.
fn public_path(public_url: &Url, path: &str) -> Url {
    let mut base = public_url.clone();
    if !base.path().ends_with('/') {
        let with_slash = format!("{}/", base.path());
        base.set_path(&with_slash);
    }
    base.join(path).unwrap_or(base)
}

/// Opens the database at `url` and applies the pending migrations. The error says why (see
/// [`cannot_open`]).
pub async fn open_database(url: &str) -> Result<DatabaseConnection, String> {
    let failed = |error| cannot_open(url, &error);
    let database = recto_domain::database::connect(url).await.map_err(failed)?;
    recto_domain::database::migrate(&database)
        .await
        .map_err(failed)?;
    Ok(database)
}

/// The message of a database that cannot be opened: the URL, without its password, and the
/// setting it comes from, so the operator knows what to fix.
pub fn cannot_open(url: &str, error: &sea_orm::DbErr) -> String {
    format!(
        "cannot open the database {} (RECTO_DATABASE_URL): {error}",
        without_password(url)
    )
}

/// `url` with its password left out, for messages and logs. A URL that does not parse is shown
/// up to its scheme only, since where its password is cannot be told.
pub fn without_password(url: &str) -> String {
    match Url::parse(url) {
        Ok(mut parsed) if parsed.password().is_some() => {
            // Setting the password only fails on URLs that cannot have one.
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Ok(_) => url.to_owned(),
        Err(_) => match url.split_once(':') {
            Some((scheme, _)) => format!("{scheme}:…"),
            None => "(unreadable URL)".to_owned(),
        },
    }
}

/// The directory of scan results, private to recto. Results of a previous run are unreachable
/// (they were only indexed in memory), so it starts empty.
pub fn results_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("recto-scans");
    match std::fs::remove_dir_all(&dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::fs::create_dir_all(&dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Builds every service. Nothing here needs CUPS, a scanner or the identity provider to answer,
/// so recto starts while they are down.
pub async fn build(
    config: &Config,
    leptos_options: leptos::config::LeptosOptions,
    database: DatabaseConnection,
    branding: Branding,
) -> (AppState, AuthState) {
    let dir = results_dir().unwrap_or_else(|error| {
        // Scanning reports the storage error when it is used; printing must not suffer.
        let dir = std::env::temp_dir().join("recto-scans");
        tracing::warn!(%error, dir = %dir.display(), "the scan results directory is not usable");
        dir
    });
    let results = ResultStore::new(dir, SCAN_RESULT_TTL);
    let state = AppState {
        print: Arc::new(Cups::new(config.cups_url.clone())),
        scan: scanners().await,
        results: Arc::new(results),
        output_dir: config.scan_output_dir.clone(),
        max_upload_size: config.max_upload_size,
        leptos_options,
        database: database.clone(),
        branding: Arc::new(branding),
        job_visibility: config.job_visibility,
    };
    let openid = config
        .oidc_issuer
        .clone()
        .zip(config.oidc_client_id.clone())
        .map(|(issuer, client_id)| {
            Arc::new(OpenId::new(OpenIdSettings {
                issuer,
                client_id,
                redirect_url: public_path(&config.public_url, "auth/callback"),
                extra_scopes: config.oidc_scopes.clone(),
                required_claim: config.oidc_required_claim.clone(),
                required_value: config.oidc_required_value.clone(),
                administrator_claim: config.oidc_administrator_claim.clone(),
                administrator_value: config.oidc_administrator_value.clone(),
                groups_claim: config.oidc_groups_claim.clone(),
            }))
        });
    let auth = AuthState {
        local_accounts: config.local_accounts,
        password_policy: PasswordPolicy {
            minimum_length: config.password_minimum_length,
        },
        trust_forwarded_headers: config.trust_forwarded_headers,
        ..AuthState::new(openid, config.public_url.clone(), database)
    };
    (state, auth)
}

/// The SANE scanners, or a stand-in reporting why SANE could not start: printing must keep
/// working on a machine without scanners or SANE configuration.
async fn scanners() -> Arc<dyn ScannerService> {
    // `sane_init` blocks while backends load.
    match tokio::task::spawn_blocking(SaneScanners::start).await {
        Ok(Ok(scanners)) => Arc::new(scanners),
        Ok(Err(error)) => {
            tracing::warn!(%error, "SANE could not start; scanning is off");
            Arc::new(NoScanners(error.to_string()))
        }
        Err(error) => {
            tracing::warn!(%error, "SANE crashed while starting; scanning is off");
            Arc::new(NoScanners("SANE crashed while starting".into()))
        }
    }
}

/// Answers every request with the reason SANE did not start.
struct NoScanners(String);

impl NoScanners {
    fn error(&self) -> ScanError {
        ScanError::Device(self.0.clone())
    }
}

#[async_trait]
impl ScannerService for NoScanners {
    async fn scanners(&self, _refresh: bool) -> Result<Vec<ScannerInfo>, ScanError> {
        Err(self.error())
    }

    async fn capabilities(&self, _id: &str) -> Result<ScannerCapabilities, ScanError> {
        Err(self.error())
    }

    async fn scan(&self, _id: &str, _request: ScanRequest) -> Result<Vec<ScannedPage>, ScanError> {
        Err(self.error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn auth(public_url: &str) -> AuthState {
        let url = Url::parse(public_url).unwrap();
        AuthState::new(None, url, recto_domain::testing::database().await)
    }

    #[tokio::test]
    async fn secure_cookies_follow_the_public_url() {
        assert!(auth("https://recto.example.org").await.secure_cookies);
        assert!(!auth("http://192.168.1.2:8080").await.secure_cookies);
    }

    #[tokio::test]
    async fn public_paths_keep_a_prefix() {
        assert_eq!(
            auth("https://example.org/recto")
                .await
                .public("auth/callback")
                .as_str(),
            "https://example.org/recto/auth/callback"
        );
        assert_eq!(
            auth("https://example.org")
                .await
                .public("auth/callback")
                .as_str(),
            "https://example.org/auth/callback"
        );
    }

    #[test]
    fn database_urls_are_shown_without_their_password() {
        assert_eq!(
            without_password("postgres://recto:secret@db:5432/recto"),
            "postgres://recto@db:5432/recto"
        );
        assert_eq!(
            without_password("sqlite:///data/recto.db"),
            "sqlite:///data/recto.db"
        );
        assert_eq!(
            without_password("sqlite://target/dev.db"),
            "sqlite://target/dev.db"
        );
        assert_eq!(
            without_password("postgres://recto:secret@db:port/recto"),
            "postgres:…"
        );
        assert_eq!(without_password("recto.db"), "(unreadable URL)");
    }

    #[tokio::test]
    async fn no_scanners_reports_why() {
        let scanners = NoScanners("no backend".into());
        assert_eq!(
            scanners.scanners(true).await.unwrap_err(),
            ScanError::Device("no backend".into())
        );
    }
}
