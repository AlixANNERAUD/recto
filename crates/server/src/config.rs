//! Command line and environment configuration.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
};

use getargs_derive::{GetArgs, Subcommand, ValueEnum};
use ipnet::{IpNet, Ipv4Net};
use recto_app::state::JobVisibility;
use recto_domain::passwords;
use recto_printing::printer::Media;
use url::Url;

use crate::{branding::BrandOptions, dnssd};

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

/// The settings of `recto serve`. Each is also an environment variable named `RECTO_<SETTING>`.
// A setting whose mistake needs a message naming its variable is read as text and checked when
// it becomes a `Config`, since the parser drops a value parser's own message. The defaults are
// literals because the help is written at compile time; the tests check they match the
// constants.
#[derive(Clone, Debug, GetArgs)]
pub struct ServeArguments {
    #[arg(long, env = "RECTO_PUBLIC_URL")]
    pub public_url: String,
    /// `sqlite://<path>` (created if missing) or `postgres://<user>:<password>@<host>/<name>`
    /// (must exist). Migrations run at startup.
    #[arg(
        long,
        env = "RECTO_DATABASE_URL",
        default_value = "sqlite:///data/recto.db"
    )]
    pub database_url: String,
    /// The issuer URL of the OpenID Connect provider; set together with the client id, or
    /// neither for local accounts only.
    #[arg(long, env = "RECTO_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,
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
    #[arg(long, env = "RECTO_LOCAL_ACCOUNTS", default_value = "true")]
    pub local_accounts: String,
    /// Minimum length of local account passwords.
    #[arg(long, env = "RECTO_PASSWORD_MINIMUM_LENGTH", default_value = "12")]
    pub password_minimum_length: usize,
    /// Read the client address from `X-Forwarded-For`, `X-Real-IP` or `Forwarded`; only behind
    /// a proxy you control.
    #[arg(long, env = "RECTO_TRUST_FORWARDED_HEADERS", default_value = "false")]
    pub trust_forwarded_headers: String,
    /// Computers and phones print and scan through recto with device passwords; `false`
    /// removes their routes and the Account page's Devices section.
    #[arg(long, env = "RECTO_OS_INTEGRATION", default_value = "true")]
    pub os_integration: String,
    /// Accept device passwords over plain HTTP although the public URL is `http`; loopback
    /// clients always may.
    #[arg(long, env = "RECTO_IPP_ALLOW_PLAIN", default_value = "false")]
    pub ipp_allow_plain: String,
    /// Where recto keeps the documents computers and phones send, until they are released.
    #[arg(long, env = "RECTO_DATA_DIR", default_value = "/data")]
    pub data_dir: PathBuf,
    /// How many hours a held job waits for its owner before it is deleted, 1 to 720.
    #[arg(long, env = "RECTO_HELD_JOB_HOURS", default_value = "24")]
    pub held_job_hours: String,
    /// The paper size recto's printer offers devices by default, as a PWG media name.
    #[arg(long, env = "RECTO_DEFAULT_MEDIA", default_value = "iso_a4_210x297mm")]
    pub default_media: String,
    /// The networks whose devices may scan without credentials on the LAN listener, on the
    /// scanners open to everyone: CIDRs, comma separated. Empty by default.
    #[arg(long, env = "RECTO_ESCL_OPEN_NETWORKS", default_value = "")]
    pub escl_open_networks: String,
    /// Where the LAN listener takes TLS connections from the devices that found recto through
    /// DNS-SD, such as `[::]:8631`; unset or blank leaves it off.
    #[arg(long, env = "RECTO_LAN_LISTEN_ADDRESS")]
    pub lan_listen_address: Option<String>,
    /// The LAN listener's certificate chain, PEM; with `lan_tls_key`, or neither for a
    /// self-signed certificate kept in the data directory.
    #[arg(long, env = "RECTO_LAN_TLS_CERTIFICATE")]
    pub lan_tls_certificate: Option<PathBuf>,
    /// The private key of `lan_tls_certificate`, PEM.
    #[arg(long, env = "RECTO_LAN_TLS_KEY")]
    pub lan_tls_key: Option<PathBuf>,
    /// Announce the LAN listener's printer and scanners over multicast DNS; needs the LAN
    /// listener.
    #[arg(long, env = "RECTO_DNSSD", default_value = "false")]
    pub dnssd: String,
    /// The name Print dialogs show; the brand name when unset.
    #[arg(long, env = "RECTO_DNSSD_NAME")]
    pub dnssd_name: Option<String>,
    /// The network interfaces to announce on, comma separated; when empty, every interface but
    /// loopback, Docker's, virtual machines' and VPNs'.
    #[arg(long, env = "RECTO_DNSSD_INTERFACES", default_value = "")]
    pub dnssd_interfaces: String,
    #[arg(long, env = "RECTO_CUPS_URL", default_value = "http://localhost:631")]
    pub cups_url: String,
    /// CUPS's Unix socket, which lets administrators manage printers; unset or blank turns
    /// printer administration off.
    #[arg(long, env = "RECTO_CUPS_ADMIN_SOCKET")]
    pub cups_admin_socket: Option<PathBuf>,
    #[arg(long, env = "RECTO_SCAN_OUTPUT_DIR")]
    pub scan_output_dir: Option<PathBuf>,
    /// The largest document accepted, in bytes: 50 MiB by default.
    #[arg(long, env = "RECTO_MAX_UPLOAD_SIZE", default_value = "52428800")]
    pub max_upload_size: usize,
    #[arg(long, env = "RECTO_LISTEN_ADDRESS", default_value = "0.0.0.0:8080")]
    pub listen_address: SocketAddr,
    #[arg(long, env = "RECTO_SITE_ROOT", default_value = "/app/site")]
    pub site_root: PathBuf,
    /// `all` shows every job to everyone; `own` shows people the jobs they submitted through
    /// recto, and administrators every job.
    #[arg(long, env = "RECTO_JOB_VISIBILITY", default_value = "all")]
    pub job_visibility: String,
    /// The ISO 4217 code costs are shown in; display only.
    #[arg(long, env = "RECTO_CURRENCY", default_value = "EUR")]
    pub currency: String,
    /// The IANA time zone whose calendar the quota periods follow.
    #[arg(long, env = "RECTO_TIMEZONE", default_value = "UTC")]
    pub timezone: String,
    #[arg(long, env = "RECTO_LOG_FORMAT", value_enum, default_value = "json")]
    pub log_format: LogFormat,
    /// The name, colors, logo and icons; checked when the server starts (`branding::load`).
    #[command(flatten)]
    pub brand: BrandOptions,
}

/// What `recto serve` runs with: [`ServeArguments`] parsed and checked.
#[derive(Clone, Debug)]
pub struct Config {
    pub public_url: Url,
    pub database_url: String,
    /// The OpenID Connect provider; set together with `oidc_client_id`, or neither.
    pub oidc_issuer: Option<Url>,
    pub oidc_client_id: Option<String>,
    pub oidc_scopes: Vec<String>,
    pub oidc_required_claim: Option<String>,
    pub oidc_required_value: Option<String>,
    pub oidc_administrator_claim: Option<String>,
    pub oidc_administrator_value: Option<String>,
    pub oidc_groups_claim: Option<String>,
    pub local_accounts: bool,
    pub password_minimum_length: usize,
    pub trust_forwarded_headers: bool,
    pub os_integration: bool,
    pub ipp_allow_plain: bool,
    pub data_dir: PathBuf,
    pub held_job_hours: u32,
    pub default_media: Media,
    pub escl_open_networks: OpenNetworks,
    pub lan_listen_address: Option<SocketAddr>,
    pub lan_tls_certificate: Option<PathBuf>,
    pub lan_tls_key: Option<PathBuf>,
    pub dnssd: bool,
    pub dnssd_name: Option<String>,
    pub dnssd_interfaces: Vec<String>,
    pub cups_url: Url,
    pub cups_admin_socket: Option<PathBuf>,
    pub scan_output_dir: Option<PathBuf>,
    pub max_upload_size: usize,
    pub listen_address: SocketAddr,
    pub site_root: PathBuf,
    pub job_visibility: JobVisibility,
    pub currency: String,
    pub timezone: chrono_tz::Tz,
    pub log_format: LogFormat,
    pub brand: BrandOptions,
}

impl TryFrom<ServeArguments> for Config {
    type Error = String;

    /// Drops blank optional values (a variable set to nothing means the same as no variable),
    /// parses the settings read as text, then refuses combinations that would start recto
    /// wrongly configured. The message names the variable to fix.
    fn try_from(arguments: ServeArguments) -> Result<Self, String> {
        let ServeArguments {
            public_url,
            database_url,
            oidc_issuer,
            oidc_client_id,
            mut oidc_scopes,
            oidc_required_claim,
            oidc_required_value,
            oidc_administrator_claim,
            oidc_administrator_value,
            oidc_groups_claim,
            local_accounts,
            password_minimum_length,
            trust_forwarded_headers,
            os_integration,
            ipp_allow_plain,
            data_dir,
            held_job_hours,
            default_media,
            escl_open_networks,
            lan_listen_address,
            lan_tls_certificate,
            lan_tls_key,
            dnssd,
            dnssd_name,
            dnssd_interfaces,
            cups_url,
            cups_admin_socket,
            scan_output_dir,
            max_upload_size,
            listen_address,
            site_root,
            job_visibility,
            currency,
            timezone,
            log_format,
            mut brand,
        } = arguments;
        oidc_scopes.retain(|scope| !scope.trim().is_empty());
        brand.brand_short_name = given(brand.brand_short_name);
        brand.brand_logo = given_path(brand.brand_logo);
        brand.brand_icon = given_path(brand.brand_icon);
        let oidc_issuer = match given(oidc_issuer) {
            Some(text) => Some(
                Url::parse(text.trim())
                    .map_err(|error| format!("RECTO_OIDC_ISSUER is not a valid URL: {error}"))?,
            ),
            None => None,
        };
        let oidc_client_id = given(oidc_client_id);
        // Half a provider would quietly turn single sign-on off.
        if oidc_issuer.is_some() != oidc_client_id.is_some() {
            return Err(
                "RECTO_OIDC_ISSUER and RECTO_OIDC_CLIENT_ID go together: set both to \
                        sign in with OpenID Connect, or neither"
                    .to_owned(),
            );
        }
        let local_accounts = boolean("RECTO_LOCAL_ACCOUNTS", &local_accounts)?;
        if !local_accounts && oidc_issuer.is_none() {
            return Err(
                "RECTO_LOCAL_ACCOUNTS is false and OpenID Connect is not configured, so \
                        nobody could sign in: set RECTO_OIDC_ISSUER and RECTO_OIDC_CLIENT_ID, or \
                        turn local accounts back on"
                    .to_owned(),
            );
        }
        let oidc_required_claim = given(oidc_required_claim);
        let oidc_required_value = given(oidc_required_value);
        // The value alone would be ignored: a misspelt claim variable would let every account
        // of the provider in, without a word.
        if oidc_required_value.is_some() && oidc_required_claim.is_none() {
            return Err(
                "RECTO_OIDC_REQUIRED_VALUE is set without RECTO_OIDC_REQUIRED_CLAIM: set \
                        the claim the value applies to, or remove the value"
                    .to_owned(),
            );
        }
        let oidc_administrator_claim = given(oidc_administrator_claim);
        let oidc_administrator_value = given(oidc_administrator_value);
        // Likewise, the value alone would grant administration to nobody, without a word.
        if oidc_administrator_value.is_some() && oidc_administrator_claim.is_none() {
            return Err("RECTO_OIDC_ADMINISTRATOR_VALUE is set without \
                        RECTO_OIDC_ADMINISTRATOR_CLAIM: set the claim the value applies to, or \
                        remove the value"
                .to_owned());
        }
        check_password_minimum_length(password_minimum_length)?;
        let lan_listen_address = match given(lan_listen_address) {
            Some(text) => Some(text.trim().parse().map_err(|_| {
                format!(
                    "RECTO_LAN_LISTEN_ADDRESS must be an address and a port, such as [::]:8631 \
                     or 0.0.0.0:8631, not {text}"
                )
            })?),
            None => None,
        };
        let lan_tls_certificate = given_path(lan_tls_certificate);
        let lan_tls_key = given_path(lan_tls_key);
        // One file alone would quietly fall back to the self-signed certificate.
        if lan_tls_certificate.is_some() != lan_tls_key.is_some() {
            return Err(
                "RECTO_LAN_TLS_CERTIFICATE and RECTO_LAN_TLS_KEY go together: set both \
                        to use your own certificate on the LAN listener, or neither for a \
                        self-signed one"
                    .to_owned(),
            );
        }
        let os_integration = boolean("RECTO_OS_INTEGRATION", &os_integration)?;
        let dnssd = boolean("RECTO_DNSSD", &dnssd)?;
        // The services announced point at the LAN listener: without it there is nothing to find.
        if dnssd && lan_listen_address.is_none() {
            return Err(
                "RECTO_DNSSD is true but RECTO_LAN_LISTEN_ADDRESS is not set: DNS-SD \
                        announces the LAN listener, so set it too, such as [::]:8631"
                    .to_owned(),
            );
        }
        // Its printer and scanners answer 404 when OS integration is off.
        if dnssd && !os_integration {
            return Err(
                "RECTO_DNSSD is true but RECTO_OS_INTEGRATION is false: DNS-SD would \
                        announce a printer and scanners that are turned off, so turn OS \
                        integration on or DNS-SD off"
                    .to_owned(),
            );
        }
        let dnssd_name = given(dnssd_name).map(|name| name.trim().to_owned());
        if let Some(name) = &dnssd_name
            && name.len() > dnssd::INSTANCE_MAX
        {
            return Err(format!(
                "RECTO_DNSSD_NAME must be at most {} bytes long, not {}",
                dnssd::INSTANCE_MAX,
                name.len()
            ));
        }
        let public_url = Url::parse(public_url.trim())
            .map_err(|error| format!("RECTO_PUBLIC_URL is not a valid URL: {error}"))?;
        // Routes, redirects and the cookie path all assume recto is the whole site.
        if public_url.path() != "/" {
            return Err("RECTO_PUBLIC_URL must be the root of its site, such as \
                        https://recto.example.org: recto cannot be served under a path"
                .to_owned());
        }
        Ok(Self {
            public_url,
            database_url,
            oidc_issuer,
            oidc_client_id,
            oidc_scopes,
            oidc_required_claim,
            oidc_required_value,
            oidc_administrator_claim,
            oidc_administrator_value,
            oidc_groups_claim: given(oidc_groups_claim),
            local_accounts,
            password_minimum_length,
            trust_forwarded_headers: boolean(
                "RECTO_TRUST_FORWARDED_HEADERS",
                &trust_forwarded_headers,
            )?,
            os_integration,
            ipp_allow_plain: boolean("RECTO_IPP_ALLOW_PLAIN", &ipp_allow_plain)?,
            data_dir: if data_dir.as_os_str().is_empty() {
                PathBuf::from(DEFAULT_DATA_DIR)
            } else {
                data_dir
            },
            held_job_hours: parse_held_job_hours(&held_job_hours)?,
            default_media: parse_default_media(&default_media)?,
            escl_open_networks: parse_open_networks(&escl_open_networks)?,
            lan_listen_address,
            lan_tls_certificate,
            lan_tls_key,
            dnssd,
            dnssd_name,
            dnssd_interfaces: parse_dnssd_interfaces(&dnssd_interfaces)?,
            cups_url: parse_cups_url(&cups_url)
                .map_err(|error| format!("RECTO_CUPS_URL is not a valid URL: {error}"))?,
            cups_admin_socket: given_path(cups_admin_socket),
            scan_output_dir: given_path(scan_output_dir),
            max_upload_size,
            listen_address,
            site_root,
            job_visibility: parse_job_visibility(&job_visibility)?,
            currency: parse_currency(&currency)?,
            timezone: parse_timezone(&timezone)?,
            log_format,
            brand,
        })
    }
}

/// `None` for a blank value, which an unset-but-declared environment variable produces.
fn given(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

fn given_path(path: Option<PathBuf>) -> Option<PathBuf> {
    path.filter(|path| !path.as_os_str().is_empty())
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

fn boolean(variable: &str, value: &str) -> Result<bool, String> {
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{variable} must be true or false, not {value}")),
    }
}

fn parse_job_visibility(value: &str) -> Result<JobVisibility, String> {
    match value.trim() {
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

fn parse_dnssd_interfaces(value: &str) -> Result<Vec<String>, String> {
    dnssd::parse_interfaces(value).map_err(|error| {
        format!("RECTO_DNSSD_INTERFACES must be interface names, comma separated: {error}")
    })
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

/// What `recto healthcheck` needs: where the server listens.
#[derive(Clone, Debug, GetArgs)]
pub struct Healthcheck {
    #[arg(long, env = "RECTO_LISTEN_ADDRESS", default_value = "0.0.0.0:8080")]
    pub listen_address: SocketAddr,
}

/// The command line of the binary.
#[derive(Debug, GetArgs)]
#[command(
    version,
    about = "Self-hosted web app to print and scan, with local accounts or OpenID Connect sign-in."
)]
pub struct CommandLine {
    #[command(subcommand)]
    pub command: Command,
}

/// The database every command except `healthcheck` works on.
#[derive(Clone, Debug, GetArgs)]
pub struct DatabaseOptions {
    /// `sqlite://<path>` or `postgres://<user>:<password>@<host>/<name>`.
    #[arg(
        long,
        env = "RECTO_DATABASE_URL",
        default_value = "sqlite:///data/recto.db"
    )]
    pub database_url: String,
}

#[derive(Clone, Debug, GetArgs)]
pub struct BackupOptions {
    /// Where to write the copy; it must not exist yet.
    pub file: PathBuf,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

#[derive(Clone, Debug, GetArgs)]
pub struct CreateAdministratorOptions {
    #[arg(long)]
    pub username: String,
    /// Defaults to the username.
    #[arg(long)]
    pub display_name: Option<String>,
    #[arg(long)]
    pub email: Option<String>,
    /// Minimum length of the password.
    #[arg(long, env = "RECTO_PASSWORD_MINIMUM_LENGTH", default_value = "12")]
    pub password_minimum_length: usize,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

#[derive(Clone, Debug, GetArgs)]
pub struct AdministratorOptions {
    pub username: String,
    /// Remove administration instead of granting it.
    #[arg(long)]
    pub remove: bool,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

#[derive(Clone, Debug, GetArgs)]
pub struct DnssdServicesOptions {
    /// Where to write the `.service` files, such as `/etc/avahi/services`; created if missing.
    pub dir: PathBuf,
    /// The LAN listener's port as published on the host.
    #[arg(long, default_value_t = 8631)]
    pub port: u16,
    /// The printer's UUID and the pages the records link to derive from it.
    #[arg(long, env = "RECTO_PUBLIC_URL")]
    pub public_url: Url,
    /// The name Print dialogs show; the brand name when unset.
    #[arg(long, env = "RECTO_DNSSD_NAME")]
    pub dnssd_name: Option<String>,
    #[arg(long, env = "RECTO_BRAND_NAME", default_value = "recto")]
    pub brand_name: String,
    #[command(flatten)]
    pub database: DatabaseOptions,
}

// The arguments of `serve` are large, but parsed once at startup.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the web server.
    Serve(ServeArguments),
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
    /// Write the DNS-SD records of the LAN listener as Avahi service files, for a host whose
    /// avahi-daemon announces recto while it stays on a bridge network.
    DnssdServices(DnssdServicesOptions),
}

#[cfg(test)]
mod tests {
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

    /// Parses the arguments after the program name only: the environment is ignored, so RECTO_*
    /// variables exported in a developer's shell cannot change the outcome.
    fn parse_arguments<'a>(arguments: impl IntoIterator<Item = &'a str>) -> Result<Config, String> {
        let arguments = ServeArguments::from_args_with_env(arguments.into_iter().skip(1), |_| None)
            .map_err(|error| error.to_string())?;
        Config::try_from(arguments)
    }

    fn parse(extra: &[&str]) -> Result<Config, String> {
        parse_arguments(REQUIRED.iter().chain(extra).copied())
    }

    #[test]
    fn literal_defaults_match_the_constants() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.database_url, DEFAULT_DATABASE_URL);
        assert_eq!(config.data_dir, PathBuf::from(DEFAULT_DATA_DIR));
        assert_eq!(
            config.password_minimum_length,
            passwords::DEFAULT_MINIMUM_LENGTH
        );
        assert_eq!(config.max_upload_size, 50 * 1024 * 1024);
        assert_eq!(config.brand.brand_name, crate::branding::DEFAULT_NAME);
        assert_eq!(config.brand.brand_color, crate::branding::DEFAULT_COLOR);
        let database = DatabaseOptions::from_args_with_env([].into_iter(), |_| None).unwrap();
        assert_eq!(database.database_url, DEFAULT_DATABASE_URL);
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
    fn the_lan_listener_and_dns_sd_are_off_by_default() {
        let config = parse(&[]).unwrap();
        assert_eq!(config.lan_listen_address, None);
        assert_eq!(config.lan_tls_certificate, None);
        assert_eq!(config.lan_tls_key, None);
        assert!(!config.dnssd);
        assert_eq!(config.dnssd_name, None);
        assert!(config.dnssd_interfaces.is_empty());
        // Blank is unset, as for every setting.
        let blank = parse(&[
            "--lan-listen-address",
            " ",
            "--lan-tls-certificate",
            "",
            "--lan-tls-key",
            "",
            "--dnssd-name",
            " ",
            "--dnssd-interfaces",
            "",
        ])
        .unwrap();
        assert_eq!(blank.lan_listen_address, None);
        assert_eq!(blank.lan_tls_certificate, None);
        assert_eq!(blank.dnssd_name, None);
    }

    #[test]
    fn lan_settings_are_read() {
        let config = parse(&[
            "--lan-listen-address",
            "[::]:8631",
            "--lan-tls-certificate",
            "/certs/lan.pem",
            "--lan-tls-key",
            "/certs/lan.key",
            "--dnssd",
            "true",
            "--dnssd-name",
            " Office ",
            "--dnssd-interfaces",
            "eth0, wlan0",
        ])
        .unwrap();
        assert_eq!(
            config.lan_listen_address,
            Some("[::]:8631".parse().unwrap())
        );
        assert_eq!(
            config.lan_tls_certificate,
            Some(PathBuf::from("/certs/lan.pem"))
        );
        assert_eq!(config.lan_tls_key, Some(PathBuf::from("/certs/lan.key")));
        assert!(config.dnssd);
        assert_eq!(config.dnssd_name.as_deref(), Some("Office"));
        assert_eq!(config.dnssd_interfaces, ["eth0", "wlan0"]);
    }

    #[test]
    fn wrong_lan_settings_are_refused_by_name() {
        for (arguments, variable) in [
            (
                &["--lan-listen-address", "8631"][..],
                "RECTO_LAN_LISTEN_ADDRESS",
            ),
            (&["--dnssd", "yes"][..], "RECTO_DNSSD"),
            // DNS-SD announces the LAN listener: without it there is nothing to find.
            (&["--dnssd", "true"][..], "RECTO_LAN_LISTEN_ADDRESS"),
            // Nor without OS integration, whose printer and scanners it would announce.
            (
                &[
                    "--dnssd",
                    "true",
                    "--lan-listen-address",
                    "[::]:8631",
                    "--os-integration",
                    "false",
                ][..],
                "RECTO_OS_INTEGRATION",
            ),
            (
                &[
                    "--lan-listen-address",
                    "0.0.0.0:8631",
                    "--lan-tls-certificate",
                    "/certs/lan.pem",
                ][..],
                "RECTO_LAN_TLS_KEY",
            ),
            (
                &[
                    "--lan-listen-address",
                    "0.0.0.0:8631",
                    "--lan-tls-key",
                    "/certs/lan.key",
                ][..],
                "RECTO_LAN_TLS_CERTIFICATE",
            ),
            (
                &["--dnssd-interfaces", "eth 0"][..],
                "RECTO_DNSSD_INTERFACES",
            ),
            // A DNS-SD instance name is one label of at most 63 bytes.
            (&["--dnssd-name", &"é".repeat(32)][..], "RECTO_DNSSD_NAME"),
        ] {
            let error = parse(arguments).unwrap_err();
            assert!(
                error.to_string().contains(variable),
                "{arguments:?}: {error}"
            );
        }
        assert!(parse(&["--dnssd-name", &"é".repeat(31)]).is_ok());
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
    fn parse_public_url(public_url: &str) -> Result<Config, String> {
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
