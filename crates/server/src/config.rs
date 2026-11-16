//! Command line and environment configuration.

use std::{net::SocketAddr, path::PathBuf};

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use recto_domain::passwords;
use url::Url;

use crate::branding::BrandOptions;

/// Where the database is when `RECTO_DATABASE_URL` is not set: the container's data volume.
pub const DEFAULT_DATABASE_URL: &str = "sqlite:///data/recto.db";

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
    /// `sqlite://<path>` (created if missing) or `postgres://<user>:<password>@<host>/<name>`
    /// (must exist). Migrations run at startup.
    #[arg(
        long,
        env = "RECTO_DATABASE_URL",
        default_value = DEFAULT_DATABASE_URL,
        hide_env_values = true
    )]
    pub database_url: String,
    /// The issuer URL of the OpenID Connect provider; set together with the client id, or
    /// neither for local accounts only.
    // Read as text, so that a blank variable means no provider; `Config::normalized` parses it
    // into `oidc_issuer`.
    #[arg(
        long = "oidc-issuer",
        env = "RECTO_OIDC_ISSUER",
        value_name = "OIDC_ISSUER"
    )]
    oidc_issuer_text: Option<String>,
    /// The OpenID Connect provider; set together with `oidc_client_id`, or neither.
    #[arg(skip)]
    pub oidc_issuer: Option<Url>,
    #[arg(long, env = "RECTO_OIDC_CLIENT_ID")]
    pub oidc_client_id: Option<String>,
    /// Extra scopes, separated by spaces, requested next to `openid`.
    #[arg(long, env = "RECTO_OIDC_SCOPES", value_delimiter = ' ')]
    pub oidc_scopes: Vec<String>,
    #[arg(long, env = "RECTO_OIDC_REQUIRED_CLAIM")]
    pub oidc_required_claim: Option<String>,
    #[arg(long, env = "RECTO_OIDC_REQUIRED_VALUE")]
    pub oidc_required_value: Option<String>,
    /// A claim granting administration, checked at each sign-in.
    #[arg(long, env = "RECTO_OIDC_ADMINISTRATOR_CLAIM")]
    pub oidc_administrator_claim: Option<String>,
    /// The value the administrator claim must hold; without it the claim must be present and
    /// not `false` or `null`.
    #[arg(long, env = "RECTO_OIDC_ADMINISTRATOR_VALUE")]
    pub oidc_administrator_value: Option<String>,
    /// A claim listing the user's groups.
    #[arg(long, env = "RECTO_OIDC_GROUPS_CLAIM")]
    pub oidc_groups_claim: Option<String>,
    /// Sign-in with a username or email address and a password.
    #[arg(long, env = "RECTO_LOCAL_ACCOUNTS", default_value_t = true, action = ArgAction::Set)]
    pub local_accounts: bool,
    /// Minimum length of local account passwords.
    #[arg(long, env = "RECTO_PASSWORD_MINIMUM_LENGTH", default_value_t = passwords::DEFAULT_MINIMUM_LENGTH)]
    pub password_minimum_length: usize,
    /// Read the client address from `X-Forwarded-For`, `X-Real-IP` or `Forwarded`; only behind
    /// a proxy you control.
    #[arg(
        long,
        env = "RECTO_TRUST_FORWARDED_HEADERS",
        default_value_t = false,
        action = ArgAction::Set
    )]
    pub trust_forwarded_headers: bool,
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
    /// The name, colors, logo and icons; checked when the server starts (`branding::load`).
    #[command(flatten)]
    pub brand: BrandOptions,
}

impl Config {
    /// Drops blank optional values (a variable set to nothing means the same as no variable),
    /// then refuses combinations that would start recto wrongly configured.
    pub fn normalized(mut config: Self) -> Result<Self, clap::Error> {
        config.oidc_scopes.retain(|scope| !scope.trim().is_empty());
        for value in [
            &mut config.oidc_issuer_text,
            &mut config.oidc_client_id,
            &mut config.oidc_required_claim,
            &mut config.oidc_required_value,
            &mut config.oidc_administrator_claim,
            &mut config.oidc_administrator_value,
            &mut config.oidc_groups_claim,
            &mut config.brand.brand_short_name,
        ] {
            if value.as_deref().is_some_and(|text| text.trim().is_empty()) {
                *value = None;
            }
        }
        for path in [
            &mut config.scan_output_dir,
            &mut config.brand.brand_logo,
            &mut config.brand.brand_icon,
        ] {
            if path
                .as_ref()
                .is_some_and(|path| path.as_os_str().is_empty())
            {
                *path = None;
            }
        }
        let invalid = |message: &str| {
            Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!("{message}\n"),
            ))
        };
        config.oidc_issuer = match config.oidc_issuer_text.as_deref().map(str::trim) {
            Some(text) => match Url::parse(text) {
                Ok(url) => Some(url),
                Err(error) => {
                    return invalid(&format!("RECTO_OIDC_ISSUER is not a valid URL: {error}"));
                }
            },
            None => None,
        };
        // Half a provider would quietly turn single sign-on off.
        if config.oidc_issuer.is_some() != config.oidc_client_id.is_some() {
            return invalid(
                "RECTO_OIDC_ISSUER and RECTO_OIDC_CLIENT_ID go together: set both to sign in \
                 with OpenID Connect, or neither",
            );
        }
        if !config.local_accounts && config.oidc_issuer.is_none() {
            return invalid(
                "RECTO_LOCAL_ACCOUNTS is false and OpenID Connect is not configured, so nobody \
                 could sign in: set RECTO_OIDC_ISSUER and RECTO_OIDC_CLIENT_ID, or turn local \
                 accounts back on",
            );
        }
        // The value alone would be ignored: a misspelt claim variable would let every account
        // of the provider in, without a word.
        if config.oidc_required_value.is_some() && config.oidc_required_claim.is_none() {
            return invalid(
                "RECTO_OIDC_REQUIRED_VALUE is set without RECTO_OIDC_REQUIRED_CLAIM: set the \
                 claim the value applies to, or remove the value",
            );
        }
        // Likewise, the value alone would grant administration to nobody, without a word.
        if config.oidc_administrator_value.is_some() && config.oidc_administrator_claim.is_none() {
            return invalid(
                "RECTO_OIDC_ADMINISTRATOR_VALUE is set without RECTO_OIDC_ADMINISTRATOR_CLAIM: set \
                 the claim the value applies to, or remove the value",
            );
        }
        if let Err(message) = check_password_minimum_length(config.password_minimum_length) {
            return invalid(&message);
        }
        // Routes, redirects and the cookie path all assume recto is the whole site.
        if config.public_url.path() != "/" {
            return invalid(
                "RECTO_PUBLIC_URL must be the root of its site, such as \
                 https://recto.example.org: recto cannot be served under a path",
            );
        }
        Ok(config)
    }
}

/// Refuses a minimum password length no password could meet, or one of 0, which would accept
/// an empty password.
pub fn check_password_minimum_length(length: usize) -> Result<(), String> {
    if (1..=passwords::MAXIMUM_LENGTH).contains(&length) {
        Ok(())
    } else {
        Err(format!(
            "RECTO_PASSWORD_MINIMUM_LENGTH must be between 1 and {}, not {length}",
            passwords::MAXIMUM_LENGTH
        ))
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
pub(crate) fn parse_path(value: &str) -> Result<PathBuf, std::convert::Infallible> {
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

/// The database every command except `healthcheck` works on.
#[derive(Clone, Debug, Args)]
pub struct DatabaseOptions {
    /// `sqlite://<path>` or `postgres://<user>:<password>@<host>/<name>`.
    #[arg(
        long,
        env = "RECTO_DATABASE_URL",
        default_value = DEFAULT_DATABASE_URL,
        hide_env_values = true
    )]
    pub database_url: String,
}

#[derive(Clone, Debug, Args)]
pub struct BackupOptions {
    /// Where to write the copy; it must not exist yet.
    pub file: PathBuf,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

#[derive(Clone, Debug, Args)]
pub struct CreateAdministratorOptions {
    #[arg(long)]
    pub username: String,
    /// Defaults to the username.
    #[arg(long)]
    pub display_name: Option<String>,
    #[arg(long)]
    pub email: Option<String>,
    /// Minimum length of the password.
    #[arg(long, env = "RECTO_PASSWORD_MINIMUM_LENGTH", default_value_t = passwords::DEFAULT_MINIMUM_LENGTH)]
    pub password_minimum_length: usize,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

#[derive(Clone, Debug, Args)]
pub struct AdministratorOptions {
    pub username: String,
    /// Remove administration instead of granting it.
    #[arg(long)]
    pub remove: bool,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the web server.
    Serve(Box<Config>),
    /// Exit 0 when a running server answers `/health`; for container health checks.
    Healthcheck(Healthcheck),
    /// Apply the pending database migrations and exit.
    Migrate(DatabaseOptions),
    /// Copy the SQLite database to a new file (use pg_dump for PostgreSQL).
    Backup(BackupOptions),
    /// Create a local administrator; the password is asked twice on a terminal, otherwise read
    /// from one line of standard input.
    CreateAdministrator(CreateAdministratorOptions),
    /// Grant or remove administration of an existing user.
    Administrator(AdministratorOptions),
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
        Config::from_arg_matches(&matches).and_then(Config::normalized)
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
    fn defaults_need_only_the_public_url() {
        let config =
            parse_arguments(["recto", "--public-url", "https://recto.example.com"]).unwrap();
        assert_eq!(config.database_url, "sqlite:///data/recto.db");
        assert_eq!(config.oidc_issuer, None);
        assert_eq!(config.oidc_client_id, None);
        assert!(config.local_accounts);
        assert_eq!(config.password_minimum_length, 12);
        assert!(!config.trust_forwarded_headers);
        assert_eq!(config.oidc_administrator_claim, None);
        assert_eq!(config.oidc_groups_claim, None);
    }

    #[test]
    fn openid_connect_needs_both_issuer_and_client_id() {
        let issuer_only = [
            "recto",
            "--public-url",
            "https://recto.example.com",
            "--oidc-issuer",
            "https://id.example.com",
        ];
        let client_only = [
            "recto",
            "--public-url",
            "https://recto.example.com",
            "--oidc-client-id",
            "recto",
        ];
        for arguments in [issuer_only, client_only] {
            let error = parse_arguments(arguments).unwrap_err().to_string();
            assert!(error.contains("RECTO_OIDC_ISSUER"), "{error}");
            assert!(error.contains("RECTO_OIDC_CLIENT_ID"), "{error}");
        }
        let config = parse(&[]).unwrap();
        assert_eq!(
            config.oidc_issuer.as_ref().map(Url::as_str),
            Some("https://id.example.com/")
        );
        assert_eq!(config.oidc_client_id.as_deref(), Some("recto"));
    }

    #[test]
    fn blank_issuer_and_client_id_mean_no_openid_connect() {
        let config = parse_arguments([
            "recto",
            "--public-url",
            "https://recto.example.com",
            "--oidc-issuer",
            "",
            "--oidc-client-id",
            " ",
        ])
        .unwrap();
        assert_eq!(config.oidc_issuer, None);
        assert_eq!(config.oidc_client_id, None);
        let mut arguments = REQUIRED;
        arguments[4] = "not a url";
        let error = parse_arguments(arguments).unwrap_err();
        assert!(error.to_string().contains("RECTO_OIDC_ISSUER"), "{error}");
    }

    #[test]
    fn some_way_to_sign_in_is_required() {
        let error = parse_arguments([
            "recto",
            "--public-url",
            "https://recto.example.com",
            "--local-accounts",
            "false",
        ])
        .unwrap_err();
        assert!(
            error.to_string().contains("RECTO_LOCAL_ACCOUNTS"),
            "{error}"
        );
        let config = parse(&["--local-accounts", "false"]).unwrap();
        assert!(!config.local_accounts);
    }

    #[test]
    fn administrator_value_needs_a_claim() {
        let error = parse(&["--oidc-administrator-value", "admin"]).unwrap_err();
        assert!(
            error.to_string().contains("RECTO_OIDC_ADMINISTRATOR_CLAIM"),
            "{error}"
        );
        let config = parse(&[
            "--oidc-administrator-claim",
            "roles",
            "--oidc-administrator-value",
            "admin",
            "--oidc-groups-claim",
            "groups",
        ])
        .unwrap();
        assert_eq!(config.oidc_administrator_claim.as_deref(), Some("roles"));
        assert_eq!(config.oidc_administrator_value.as_deref(), Some("admin"));
        assert_eq!(config.oidc_groups_claim.as_deref(), Some("groups"));
    }

    /// The required arguments with another public URL.
    fn parse_public_url(public_url: &str) -> Result<Config, clap::Error> {
        let mut arguments = REQUIRED;
        arguments[2] = public_url;
        parse_arguments(arguments)
    }

    #[test]
    fn required_value_needs_a_claim() {
        // A misspelt claim variable must not leave recto open to every account.
        let error = parse(&["--oidc-required-value", "recto"]).unwrap_err();
        assert!(
            error.to_string().contains("RECTO_OIDC_REQUIRED_CLAIM"),
            "{error}"
        );
        let blank_claim = parse(&[
            "--oidc-required-claim",
            " ",
            "--oidc-required-value",
            "recto",
        ]);
        assert!(blank_claim.is_err());
        let both = parse(&[
            "--oidc-required-claim",
            "roles",
            "--oidc-required-value",
            "recto",
        ]);
        assert_eq!(both.unwrap().oidc_required_value.as_deref(), Some("recto"));
    }

    #[test]
    fn password_minimum_length_is_bounded() {
        for length in ["0", "1025"] {
            let error = parse(&["--password-minimum-length", length]).unwrap_err();
            assert!(
                error.to_string().contains("RECTO_PASSWORD_MINIMUM_LENGTH"),
                "{error}"
            );
        }
        for length in [1, 1024] {
            let config = parse(&["--password-minimum-length", &length.to_string()]).unwrap();
            assert_eq!(config.password_minimum_length, length);
        }
    }

    #[test]
    fn public_url_must_be_the_root() {
        for root in ["https://recto.example.com", "https://recto.example.com/"] {
            assert!(parse_public_url(root).is_ok(), "{root}");
        }
        for prefixed in ["https://example.com/recto", "https://example.com/recto/"] {
            let error = parse_public_url(prefixed).unwrap_err();
            assert!(error.to_string().contains("RECTO_PUBLIC_URL"), "{error}");
        }
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
            "--brand-short-name",
            " ",
            "--brand-logo",
            "",
            "--brand-icon",
            "",
        ])
        .unwrap();
        assert_eq!(config.oidc_required_claim, None);
        assert_eq!(config.oidc_required_value, None);
        assert_eq!(config.scan_output_dir, None);
        assert_eq!(config.brand.brand_short_name, None);
        assert_eq!(config.brand.brand_logo, None);
        assert_eq!(config.brand.brand_icon, None);
    }

    #[test]
    fn branding_defaults_to_recto() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.brand.brand_name, "recto");
        assert_eq!(config.brand.brand_color, "#0f6cbd");
        let config = parse(&["--brand-name", "Maison", "--brand-logo", "/brand/logo.svg"]).unwrap();
        assert_eq!(config.brand.brand_name, "Maison");
        assert_eq!(
            config.brand.brand_logo.as_deref(),
            Some(std::path::Path::new("/brand/logo.svg"))
        );
    }
}
