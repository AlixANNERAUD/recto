//! Command line and environment configuration.

use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use url::Url;

/// The format of log lines on standard error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum LogFormat {
    Json,
    Text,
}

/// What `recto serve` needs. Every field is an environment variable named `RECTO_<FIELD>`.
#[derive(Clone, Debug, Parser)]
pub struct Config {
    #[arg(long, env = "RECTO_PUBLIC_URL")]
    pub public_url: Url,
    #[arg(long, env = "RECTO_OIDC_ISSUER")]
    pub oidc_issuer: Url,
    #[arg(long, env = "RECTO_OIDC_CLIENT_ID")]
    pub oidc_client_id: String,
    /// Extra scopes, separated by spaces, requested next to `openid`.
    #[arg(long, env = "RECTO_OIDC_SCOPES", value_delimiter = ' ')]
    pub oidc_scopes: Vec<String>,
    #[arg(long, env = "RECTO_OIDC_REQUIRED_CLAIM")]
    pub oidc_required_claim: Option<String>,
    #[arg(long, env = "RECTO_OIDC_REQUIRED_VALUE")]
    pub oidc_required_value: Option<String>,
    #[arg(long, env = "RECTO_CUPS_URL", default_value = "http://localhost:631", value_parser = parse_cups_url)]
    pub cups_url: Url,
    #[arg(long, env = "RECTO_SCAN_OUTPUT_DIR", value_parser = parse_path)]
    pub scan_output_dir: Option<PathBuf>,
    #[arg(long, env = "RECTO_MAX_UPLOAD_SIZE", default_value_t = 50 * 1024 * 1024)]
    pub max_upload_size: usize,
    #[arg(long, env = "RECTO_LISTEN_ADDRESS", default_value = "0.0.0.0:8080")]
    pub listen_address: SocketAddr,
    #[arg(long, env = "RECTO_SITE_ROOT", default_value = "/app/site")]
    pub site_root: PathBuf,
    #[arg(long, env = "RECTO_LOG_FORMAT", value_enum, default_value = "json")]
    pub log_format: LogFormat,
}

impl Config {
    /// Drops blank optional values: a variable set to nothing means the same as no variable.
    pub fn normalized(mut config: Self) -> Self {
        config.oidc_scopes.retain(|scope| !scope.trim().is_empty());
        for value in [
            &mut config.oidc_required_claim,
            &mut config.oidc_required_value,
        ] {
            if value.as_deref().is_some_and(|text| text.trim().is_empty()) {
                *value = None;
            }
        }
        if config
            .scan_output_dir
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            config.scan_output_dir = None;
        }
        config
    }
}

/// CUPS clients commonly write the server as `ipp://` or `ipps://`; IPP runs over HTTP, so the
/// scheme is mapped to the one the HTTP client needs.
fn parse_cups_url(value: &str) -> Result<Url, url::ParseError> {
    let value = value.trim();
    let mapped = if let Some(rest) = value.strip_prefix("ipps://") {
        format!("https://{rest}")
    } else if let Some(rest) = value.strip_prefix("ipp://") {
        format!("http://{rest}")
    } else {
        value.to_owned()
    };
    Url::parse(&mapped)
}

/// Accepts the empty string, which an unset-but-declared environment variable produces; it is
/// turned into an absent value afterwards.
fn parse_path(value: &str) -> Result<PathBuf, std::convert::Infallible> {
    Ok(PathBuf::from(value))
}

/// What `recto healthcheck` needs: where the server listens.
#[derive(Clone, Debug, Parser)]
pub struct Healthcheck {
    #[arg(long, env = "RECTO_LISTEN_ADDRESS", default_value = "0.0.0.0:8080")]
    pub listen_address: SocketAddr,
}

/// The command line of the binary.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct CommandLine {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the web server.
    Serve(Box<Config>),
    /// Exit 0 when a running server answers `/health`; for container health checks.
    Healthcheck(Healthcheck),
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, FromArgMatches};

    use super::*;

    const REQUIRED: [&str; 7] = [
        "recto",
        "--public-url",
        "https://recto.example.com",
        "--oidc-issuer",
        "https://id.example.com",
        "--oidc-client-id",
        "recto",
    ];

    /// Parses the arguments only: the environment is ignored, so RECTO_* variables exported in a
    /// developer's shell cannot change the outcome.
    fn parse_arguments<I, T>(arguments: I) -> Result<Config, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let command = Config::command().mut_args(|argument| argument.env(None::<&str>));
        let matches = command.try_get_matches_from(arguments)?;
        Config::from_arg_matches(&matches).map(Config::normalized)
    }

    fn parse(extra: &[&str]) -> Result<Config, clap::Error> {
        parse_arguments(REQUIRED.iter().chain(extra))
    }

    #[test]
    fn parses_minimal_environment() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.cups_url.as_str(), "http://localhost:631/");
        assert_eq!(config.max_upload_size, 50 * 1024 * 1024);
        assert_eq!(config.listen_address, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(config.scan_output_dir, None);
        assert!(config.oidc_scopes.is_empty());
        assert_eq!(config.oidc_required_claim, None);
    }

    #[test]
    fn splits_extra_scopes() {
        let config = parse(&["--oidc-scopes", "a b  c"]).unwrap();
        assert_eq!(config.oidc_scopes, ["a", "b", "c"]);
    }

    #[test]
    fn accepts_ipp_cups_url() {
        let plain = parse(&["--cups-url", "ipp://cups:631"]).unwrap();
        assert_eq!(plain.cups_url.as_str(), "http://cups:631/");
        let secure = parse(&["--cups-url", "ipps://cups:631"]).unwrap();
        assert_eq!(secure.cups_url.as_str(), "https://cups:631/");
    }

    #[test]
    fn rejects_missing_issuer() {
        let arguments = [
            "recto",
            "--public-url",
            "https://recto.example.com",
            "--oidc-client-id",
            "recto",
        ];
        assert!(parse_arguments(arguments).is_err());
    }

    #[test]
    fn blank_optional_values_are_absent() {
        let config = parse(&[
            "--oidc-required-claim",
            "",
            "--oidc-required-value",
            "  ",
            "--scan-output-dir",
            "",
        ])
        .unwrap();
        assert_eq!(config.oidc_required_claim, None);
        assert_eq!(config.oidc_required_value, None);
        assert_eq!(config.scan_output_dir, None);
    }
}
