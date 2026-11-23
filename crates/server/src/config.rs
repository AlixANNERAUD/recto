//! Command line and environment configuration.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use ipnet::{IpNet, Ipv4Net};
use recto_app::state::JobVisibility;
use recto_domain::passwords;
use recto_printing::printer::Media;
use url::Url;

use crate::branding::BrandOptions;

/// Where the database is when `RECTO_DATABASE_URL` is not set: the container's data volume.
pub const DEFAULT_DATABASE_URL: &str = "sqlite:///data/recto.db";
/// Where held documents are kept when `RECTO_DATA_DIR` is not set: the container's data volume.
pub const DEFAULT_DATA_DIR: &str = "/data";
/// The longest a held job may be kept, in hours: 30 days.
pub const MAX_HELD_JOB_HOURS: u32 = 720;

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
    /// Computers and phones print and scan through recto with device passwords; `false`
    /// removes their routes and the Account page's Devices section.
    #[arg(
        long,
        env = "RECTO_OS_INTEGRATION",
        default_value = "true",
        value_parser = boolean("RECTO_OS_INTEGRATION"),
        action = ArgAction::Set
    )]
    pub os_integration: bool,
    /// Accept device passwords over plain HTTP although the public URL is `http`; loopback
    /// clients always may.
    #[arg(
        long,
        env = "RECTO_IPP_ALLOW_PLAIN",
        default_value = "false",
        value_parser = boolean("RECTO_IPP_ALLOW_PLAIN"),
        action = ArgAction::Set
    )]
    pub ipp_allow_plain: bool,
    /// Where recto keeps the documents computers and phones send, until they are released.
    #[arg(long, env = "RECTO_DATA_DIR", default_value = DEFAULT_DATA_DIR, value_parser = parse_path)]
    pub data_dir: PathBuf,
    /// How many hours a held job waits for its owner before it is deleted, 1 to 720.
    #[arg(
        long,
        env = "RECTO_HELD_JOB_HOURS",
        default_value = "24",
        value_parser = parse_held_job_hours
    )]
    pub held_job_hours: u32,
    /// The paper size recto's printer offers devices by default, as a PWG media name.
    #[arg(
        long,
        env = "RECTO_DEFAULT_MEDIA",
        default_value = "iso_a4_210x297mm",
        value_parser = parse_default_media
    )]
    pub default_media: Media,
    /// The networks whose devices may scan without credentials on the LAN listener, on the
    /// scanners open to everyone: CIDRs, comma separated. Empty by default.
    #[arg(
        long,
        env = "RECTO_ESCL_OPEN_NETWORKS",
        default_value = "",
        value_parser = parse_open_networks
    )]
    pub escl_open_networks: OpenNetworks,
    #[arg(long, env = "RECTO_CUPS_URL", default_value = "http://localhost:631", value_parser = parse_cups_url)]
    pub cups_url: Url,
    /// CUPS's Unix socket, which lets administrators manage printers; unset or blank turns
    /// printer administration off.
    #[arg(long, env = "RECTO_CUPS_ADMIN_SOCKET", value_parser = parse_path)]
    pub cups_admin_socket: Option<PathBuf>,
    #[arg(long, env = "RECTO_SCAN_OUTPUT_DIR", value_parser = parse_path)]
    pub scan_output_dir: Option<PathBuf>,
    #[arg(long, env = "RECTO_MAX_UPLOAD_SIZE", default_value_t = 50 * 1024 * 1024)]
    pub max_upload_size: usize,
    #[arg(long, env = "RECTO_LISTEN_ADDRESS", default_value = "0.0.0.0:8080")]
    pub listen_address: SocketAddr,
    #[arg(long, env = "RECTO_SITE_ROOT", default_value = "/app/site")]
    pub site_root: PathBuf,
    /// `all` shows every job to everyone; `own` shows people the jobs they submitted through
    /// recto, and administrators every job.
    #[arg(
        long,
        env = "RECTO_JOB_VISIBILITY",
        default_value = "all",
        value_parser = parse_job_visibility
    )]
    pub job_visibility: JobVisibility,
    /// The ISO 4217 code costs are shown in; display only.
    #[arg(
        long,
        env = "RECTO_CURRENCY",
        default_value = "EUR",
        value_parser = parse_currency
    )]
    pub currency: String,
    /// The IANA time zone whose calendar the quota periods follow.
    #[arg(
        long,
        env = "RECTO_TIMEZONE",
        default_value = "UTC",
        value_parser = parse_timezone
    )]
    pub timezone: chrono_tz::Tz,
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
            &mut config.cups_admin_socket,
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
        if config.data_dir.as_os_str().is_empty() {
            config.data_dir = PathBuf::from(DEFAULT_DATA_DIR);
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

/// `true` or `false`, refused with the name of the variable otherwise: clap's own parser names
/// only the command line option.
fn boolean(variable: &'static str) -> impl Fn(&str) -> Result<bool, String> + Clone {
    move |value| match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{variable} must be true or false, not {value}")),
    }
}

// Not a `ValueEnum`: its error would not name the environment variable.
fn parse_job_visibility(value: &str) -> Result<JobVisibility, String> {
    match value {
        "all" => Ok(JobVisibility::All),
        "own" => Ok(JobVisibility::Own),
        _ => Err(format!(
            "RECTO_JOB_VISIBILITY must be all or own, not {value}"
        )),
    }
}

fn parse_held_job_hours(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse()
        .ok()
        .filter(|hours| (1..=MAX_HELD_JOB_HOURS).contains(hours))
        .ok_or_else(|| {
            format!(
                "RECTO_HELD_JOB_HOURS must be a number of hours from 1 to \
                 {MAX_HELD_JOB_HOURS}, not {value}"
            )
        })
}

fn parse_default_media(value: &str) -> Result<Media, String> {
    value
        .trim()
        .parse()
        .map_err(|error| format!("RECTO_DEFAULT_MEDIA is an {error}"))
}

/// Networks, by the addresses of their devices (`RECTO_ESCL_OPEN_NETWORKS`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenNetworks(Vec<IpNet>);

impl OpenNetworks {
    /// Whether `address` is in one of the networks. An IPv4 address seen as IPv6
    /// (`::ffff:a.b.c.d`) is the IPv4 address.
    pub fn contains(&self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        self.0.iter().any(|network| network.contains(&address))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// CIDRs separated by commas, such as `192.168.1.0/24, fd00::/8`; an address alone is a network
/// of that one address, and blank means none. IPv4-mapped IPv6 networks are read as IPv4, like
/// the addresses they are compared with.
pub fn parse_open_networks(value: &str) -> Result<OpenNetworks, String> {
    let network = |item: &str| -> Option<IpNet> {
        let network = item
            .parse::<IpNet>()
            .ok()
            .or_else(|| item.parse::<IpAddr>().ok().map(IpNet::from))?;
        let IpNet::V6(v6) = network else {
            return Some(network);
        };
        match (v6.addr().to_ipv4_mapped(), v6.prefix_len().checked_sub(96)) {
            (Some(v4), Some(prefix)) => Ipv4Net::new(v4, prefix).ok().map(IpNet::V4),
            _ => Some(network),
        }
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            network(item).map(|network| network.trunc()).ok_or_else(|| {
                format!(
                    "RECTO_ESCL_OPEN_NETWORKS must be networks such as 192.168.1.0/24, comma \
                     separated, not {item}"
                )
            })
        })
        .collect::<Result<_, _>>()
        .map(OpenNetworks)
}

fn parse_currency(value: &str) -> Result<String, String> {
    let code = value.trim();
    if code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        Ok(code.to_ascii_uppercase())
    } else {
        Err(format!(
            "RECTO_CURRENCY must be an ISO 4217 code of three letters, such as EUR, not {value}"
        ))
    }
}

fn parse_timezone(value: &str) -> Result<chrono_tz::Tz, String> {
    value.trim().parse().map_err(|_| {
        format!("RECTO_TIMEZONE must be an IANA time zone name, such as Europe/Paris, not {value}")
    })
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
    fn job_visibility_defaults_to_all() {
        assert_eq!(parse(&[]).unwrap().job_visibility, JobVisibility::All);
        let own = parse(&["--job-visibility", "own"]).unwrap();
        assert_eq!(own.job_visibility, JobVisibility::Own);
        let error = parse(&["--job-visibility", "mine"]).unwrap_err();
        assert!(
            error.to_string().contains("RECTO_JOB_VISIBILITY"),
            "{error}"
        );
    }

    #[test]
    fn currency_and_timezone_are_checked() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.currency, "EUR");
        assert_eq!(config.timezone, chrono_tz::UTC);
        let paris = parse(&["--timezone", "Europe/Paris", "--currency", "usd"]).unwrap();
        assert_eq!(paris.timezone, chrono_tz::Europe::Paris);
        assert_eq!(paris.currency, "USD");
        let error = parse(&["--timezone", "Mars/Base"]).unwrap_err();
        assert!(error.to_string().contains("RECTO_TIMEZONE"), "{error}");
        let error = parse(&["--currency", "EURO"]).unwrap_err();
        assert!(error.to_string().contains("RECTO_CURRENCY"), "{error}");
    }

    #[test]
    fn the_admin_socket_is_optional() {
        assert_eq!(parse(&[]).unwrap().cups_admin_socket, None);
        assert_eq!(
            parse(&["--cups-admin-socket", ""])
                .unwrap()
                .cups_admin_socket,
            None
        );
        assert_eq!(
            parse(&["--cups-admin-socket", "/run/cups/cups.sock"])
                .unwrap()
                .cups_admin_socket,
            Some(PathBuf::from("/run/cups/cups.sock"))
        );
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
        assert!(config.os_integration);
        assert!(!config.ipp_allow_plain);
        assert_eq!(config.default_media, Media::A4);
    }

    #[test]
    fn held_job_settings_are_checked() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert_eq!(config.held_job_hours, 24);
        let config = parse(&["--data-dir", "/srv/recto", "--held-job-hours", "720"]).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/srv/recto"));
        assert_eq!(config.held_job_hours, 720);
        assert_eq!(parse(&["--held-job-hours", "1"]).unwrap().held_job_hours, 1);
        // Blank means the default, as for every setting.
        let blank = parse(&["--data-dir", ""]).unwrap();
        assert_eq!(blank.data_dir, PathBuf::from("/data"));
        for hours in ["0", "721", "-1", "a day"] {
            let error = parse(&[&format!("--held-job-hours={hours}")]).unwrap_err();
            assert!(
                error.to_string().contains("RECTO_HELD_JOB_HOURS"),
                "{error}"
            );
        }
    }

    #[test]
    fn the_default_media_is_one_of_five() {
        let config = parse(&["--default-media", "na_letter_8.5x11in"]).unwrap();
        assert_eq!(config.default_media, Media::Letter);
        for media in ["letter", "iso_a6_105x148mm", ""] {
            let error = parse(&[&format!("--default-media={media}")]).unwrap_err();
            let error = error.to_string();
            assert!(error.contains("RECTO_DEFAULT_MEDIA"), "{error}");
            assert!(error.contains("iso_a4_210x297mm"), "{error}");
        }
    }

    #[test]
    fn os_integration_settings_are_booleans() {
        let config = parse(&["--os-integration", "false", "--ipp-allow-plain", "true"]).unwrap();
        assert!(!config.os_integration);
        assert!(config.ipp_allow_plain);
        for (option, variable) in [
            ("--os-integration", "RECTO_OS_INTEGRATION"),
            ("--ipp-allow-plain", "RECTO_IPP_ALLOW_PLAIN"),
        ] {
            let error = parse(&[option, "maybe"]).unwrap_err();
            assert!(error.to_string().contains(variable), "{error}");
        }
    }

    #[test]
    fn open_networks_are_cidrs() {
        assert!(parse(&[]).unwrap().escl_open_networks.is_empty());
        assert!(
            parse(&["--escl-open-networks", " "])
                .unwrap()
                .escl_open_networks
                .is_empty()
        );
        let config = parse(&[
            "--escl-open-networks",
            "192.168.1.0/24, fd00::/8,10.0.0.7,::ffff:172.16.0.0/112",
        ])
        .unwrap();
        let networks = config.escl_open_networks;
        let address = |text: &str| text.parse::<IpAddr>().unwrap();
        for inside in [
            "192.168.1.20",
            "::ffff:192.168.1.20",
            "fd00::1",
            "10.0.0.7",
            "172.16.3.4",
        ] {
            assert!(networks.contains(address(inside)), "{inside}");
        }
        for outside in ["192.168.2.1", "10.0.0.8", "fe80::1", "172.17.0.1"] {
            assert!(!networks.contains(address(outside)), "{outside}");
        }
        for wrong in ["192.168.1.0/33", "lan", "192.168.1.0/24;10.0.0.0/8"] {
            let error = parse(&["--escl-open-networks", wrong]).unwrap_err();
            assert!(
                error.to_string().contains("RECTO_ESCL_OPEN_NETWORKS"),
                "{error}"
            );
        }
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
