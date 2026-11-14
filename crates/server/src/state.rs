//! Building the services from the configuration.

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use recto_app::state::AppState;
use recto_authentication::{OpenId, OpenIdSettings};
use recto_printing::Cups;
use recto_scanning::{
    ResultStore, SaneScanners, ScanError, ScanSettings, ScannedPage, ScannerCapabilities,
    ScannerInfo, ScannerService,
};
use url::Url;

use crate::config::Config;

/// How long a finished scan waits to be downloaded or saved.
pub const SCAN_RESULT_TTL: Duration = Duration::from_secs(60 * 60);

/// What the sign-in routes need.
#[derive(Clone)]
pub struct AuthState {
    pub openid: Arc<OpenId>,
    /// The address users reach recto at, which CSRF checks compare `Origin` against.
    pub public_url: Url,
    /// Whether the session cookie is `Secure`: browsers drop such cookies over plain HTTP.
    pub secure_cookies: bool,
}

impl AuthState {
    pub fn new(openid: Arc<OpenId>, public_url: Url) -> Self {
        let secure_cookies = public_url.scheme() == "https";
        Self {
            openid,
            public_url,
            secure_cookies,
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
    };
    let redirect_url = public_path(&config.public_url, "auth/callback");
    let openid = OpenId::new(OpenIdSettings {
        issuer: config.oidc_issuer.clone(),
        client_id: config.oidc_client_id.clone(),
        redirect_url,
        extra_scopes: config.oidc_scopes.clone(),
        required_claim: config.oidc_required_claim.clone(),
        required_value: config.oidc_required_value.clone(),
    });
    let auth = AuthState::new(Arc::new(openid), config.public_url.clone());
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

    async fn scan(
        &self,
        _id: &str,
        _settings: ScanSettings,
    ) -> Result<Vec<ScannedPage>, ScanError> {
        Err(self.error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(public_url: &str) -> AuthState {
        let url = Url::parse(public_url).unwrap();
        AuthState::new(
            Arc::new(OpenId::new(OpenIdSettings {
                issuer: Url::parse("http://127.0.0.1:9/").unwrap(),
                client_id: "recto".into(),
                redirect_url: url.clone(),
                extra_scopes: vec![],
                required_claim: None,
                required_value: None,
            })),
            url,
        )
    }

    #[test]
    fn secure_cookies_follow_the_public_url() {
        assert!(auth("https://recto.example.org").secure_cookies);
        assert!(!auth("http://192.168.1.2:8080").secure_cookies);
    }

    #[test]
    fn public_paths_keep_a_prefix() {
        assert_eq!(
            auth("https://example.org/recto")
                .public("auth/callback")
                .as_str(),
            "https://example.org/recto/auth/callback"
        );
        assert_eq!(
            auth("https://example.org").public("auth/callback").as_str(),
            "https://example.org/auth/callback"
        );
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
