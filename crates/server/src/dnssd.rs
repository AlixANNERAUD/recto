//! DNS-SD: what recto advertises on the local network so Print dialogs and scan apps find it.
//!
//! recto announces its own host name, `recto-<hostname>.local`, never the host's
//! `<hostname>.local`: the host's own responder (Avahi) owns that name, and records for it with
//! other data start a conflict that can rename the host. The records point at the LAN listener.
//!
//! Two ways out: the built-in responder ([`announce`], over `mdns-sd`, with host networking), or
//! Avahi `.service` files for the host's avahi-daemon when recto stays on a bridge network
//! ([`avahi_files`]). Only [`MdnsRegistry`] sends multicast; tests register with a fake.
//!
//! The printer is always announced. Scanners are announced when they are open to everyone:
//! the scan apps that find them this way send no password, so only those can work for them,
//! from the networks of `RECTO_ESCL_OPEN_NETWORKS`.

use std::{collections::BTreeSet, fmt::Write as _, io, time::Duration};

use mdns_sd::{IfKind, ServiceDaemon, ServiceInfo};
use recto_domain::access::{self, DeviceKind};
use recto_printing::printer::{DOCUMENT_FORMATS, URF_SUPPORTED, VirtualPrinter};
use recto_scanning::{ScannerCapabilities, ScannerInfo, ScannerService, escl};
use sea_orm::DatabaseConnection;
use url::Url;
use uuid::Uuid;

/// The longest service instance name, in bytes: one DNS label.
pub const INSTANCE_MAX: usize = 63;

/// What is advertised, fed by the printer and scanner descriptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertised {
    /// The service instance name people see in the Print dialog (`RECTO_DNSSD_NAME`).
    pub instance: String,
    /// The `.local.` host name the services point at, from [`advertised_host`].
    pub host: String,
    /// The LAN listener's port, or for Avahi files the port published on the host.
    pub port: u16,
    /// Whether the listener speaks TLS: `_ipps` and `_uscans`, else `_ipp` and `_uscan`.
    pub tls: bool,
    /// recto's public address, for the pages in `adminurl`.
    pub public_url: Url,
    /// The queue's `printer-uuid`, [`recto_printing::printer::VirtualPrinter::uuid_for`].
    pub printer_uuid: Uuid,
    /// The document formats the queue takes, as MIME types.
    pub formats: Vec<String>,
    /// The queue's `urf-supported` values; empty leaves `URF` out, as long as Apple raster is not
    /// accepted.
    pub urf: Vec<String>,
    pub scanners: Vec<AdvertisedScanner>,
}

/// One scanner, as its eSCL capabilities describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvertisedScanner {
    /// [`escl::scanner_key`], the scanner's segment in eSCL paths.
    pub key: String,
    /// The instance name: the scanner's label.
    pub name: String,
    /// [`escl::scanner_uuid`].
    pub uuid: Uuid,
    /// Vendor and model, after `recto` in `ty`.
    pub make_and_model: String,
    pub color: bool,
    pub grayscale: bool,
    pub binary: bool,
    pub platen: bool,
    pub adf: bool,
    pub duplex: bool,
}

/// recto's printer, as the IPP routes describe it, without scanners: the queue's formats and
/// Apple raster values, its UUID, over TLS on the LAN listener's `port`. `instance` is cut to
/// [`INSTANCE_MAX`] bytes.
pub fn printer(instance: &str, host: &str, port: u16, public_url: &Url) -> Advertised {
    Advertised {
        instance: cut(instance.trim(), INSTANCE_MAX).to_owned(),
        host: host.to_owned(),
        port,
        tls: true,
        public_url: public_url.clone(),
        printer_uuid: VirtualPrinter::uuid_for(public_url.as_str()),
        formats: DOCUMENT_FORMATS
            .iter()
            .map(|&format| format.to_owned())
            .collect(),
        urf: URF_SUPPORTED
            .iter()
            .map(|&value| value.to_owned())
            .collect(),
        scanners: Vec::new(),
    }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The bytes a scanner's own name keeps, however long recto's instance name is.
const LABEL_MIN: usize = 16;

/// A scanner's instance name: `<vendor> <model> (<instance>)`, so that it differs from the name
/// a network scanner announces itself under, with `tag` after the model when two scanners share
/// it. At most [`INSTANCE_MAX`] bytes: recto's instance name is cut to leave the scanner
/// 16 bytes, then the vendor and model are, never the tag.
pub fn scanner_instance(info: &ScannerInfo, instance: &str, tag: Option<&str>) -> String {
    let tag = tag.map(|tag| format!(" {tag}")).unwrap_or_default();
    let room = INSTANCE_MAX.saturating_sub(LABEL_MIN + tag.len() + " ()".len());
    let suffix = format!(" ({})", cut(instance.trim(), room).trim_end());
    let room = INSTANCE_MAX.saturating_sub(suffix.len() + tag.len());
    format!("{}{tag}{suffix}", cut(&info.label(), room).trim_end())
}

/// A scanner as its eSCL capabilities describe it, announced as `name`.
pub fn advertised_scanner(
    info: &ScannerInfo,
    caps: &ScannerCapabilities,
    public_url: &Url,
    name: String,
) -> AdvertisedScanner {
    let offered = escl::offered(caps);
    AdvertisedScanner {
        key: escl::scanner_key(&info.id),
        name,
        uuid: escl::scanner_uuid(public_url.as_str(), &info.id),
        make_and_model: info.label(),
        color: offered.color,
        grayscale: offered.grayscale,
        binary: offered.binary,
        platen: offered.platen,
        adf: offered.adf,
        duplex: offered.duplex,
    }
}

/// The scanners open to everyone whose capabilities can be read, named after `instance`.
/// Nothing is announced when the list or the access rules cannot be read: a scanner restricted
/// to some groups must not be announced by mistake.
pub async fn open_scanners(
    scan: &dyn ScannerService,
    database: &DatabaseConnection,
    public_url: &Url,
    instance: &str,
) -> Vec<AdvertisedScanner> {
    let scanners = match scan.scanners(false).await {
        Ok(scanners) => scanners,
        Err(error) => {
            tracing::warn!(%error, "listing the scanners to announce failed");
            return Vec::new();
        }
    };
    let rules = match access::rules(database, DeviceKind::Scanner).await {
        Ok(rules) => rules,
        Err(error) => {
            tracing::error!(%error, "reading the scanner access rules to announce failed");
            return Vec::new();
        }
    };
    let open = scanners
        .into_iter()
        .filter(|scanner| access::allows(rules.get(&scanner.id), false, &[]));
    let mut announced: Vec<AdvertisedScanner> = Vec::new();
    for scanner in open {
        let caps = match scan.capabilities(&scanner.id).await {
            Ok(caps) => caps,
            Err(error) => {
                tracing::warn!(%error, scanner = scanner.id, "a scanner is not announced");
                continue;
            }
        };
        let mut name = scanner_instance(&scanner, instance, None);
        // A second scanner of one model: the key tells it apart, as its eSCL address does.
        if announced.iter().any(|known| known.name == name) {
            let key = escl::scanner_key(&scanner.id);
            name = scanner_instance(&scanner, instance, Some(&key));
        }
        announced.push(advertised_scanner(&scanner, &caps, public_url, name));
    }
    announced
}

/// How long the scanners may take to be listed and described before the printer is announced
/// without them.
const SCANNER_WAIT: Duration = Duration::from_secs(60);

/// Announces `printer` and the scanners open to everyone on `registry`. SANE is told the
/// scanners' names first, so that it never lists them back if a network backend finds them.
pub async fn announce<R: Registry + Send + 'static>(
    mut printer: Advertised,
    scan: &dyn ScannerService,
    database: &DatabaseConnection,
    registry: R,
) -> Result<Responder<R>, DnssdError> {
    let scanners = open_scanners(scan, database, &printer.public_url, &printer.instance);
    // SANE's network backends can take long to answer: the printer must not wait for ever.
    printer.scanners = tokio::time::timeout(SCANNER_WAIT, scanners)
        .await
        .unwrap_or_else(|_| {
            tracing::warn!("the scanners did not answer in time and are not announced");
            Vec::new()
        });
    scan.hide_advertised(
        printer
            .scanners
            .iter()
            .map(|scanner| scanner.name.clone())
            .collect(),
    );
    // A service that fails makes `start` unregister the others, which waits for their goodbyes:
    // never on the runtime's threads.
    tokio::task::spawn_blocking(move || start(&printer, registry))
        .await
        .unwrap_or_else(|panicked| std::panic::resume_unwind(panicked.into_panic()))
}

/// The DNS-SD service types recto can announce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceType {
    Ipp,
    Ipps,
    Uscan,
    Uscans,
}

impl ServiceType {
    pub fn printer(tls: bool) -> Self {
        if tls { Self::Ipps } else { Self::Ipp }
    }

    pub fn scanner(tls: bool) -> Self {
        if tls { Self::Uscans } else { Self::Uscan }
    }

    /// The type without its domain, as Avahi files write it: `_ipps._tcp`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ipp => "_ipp._tcp",
            Self::Ipps => "_ipps._tcp",
            Self::Uscan => "_uscan._tcp",
            Self::Uscans => "_uscans._tcp",
        }
    }

    pub fn tls(self) -> bool {
        matches!(self, Self::Ipps | Self::Uscans)
    }

    pub fn is_printer(self) -> bool {
        matches!(self, Self::Ipp | Self::Ipps)
    }

    /// The subtypes of a printer: `_universal`, which iOS and macOS browse, then `_print`, the
    /// IPP Everywhere one. Scanners have none.
    pub fn subtypes(self) -> Vec<String> {
        if self.is_printer() {
            ["_universal", "_print"]
                .iter()
                .map(|subtype| format!("{subtype}._sub.{}", self.name()))
                .collect()
        } else {
            Vec::new()
        }
    }
}

/// One service instance to announce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub kind: ServiceType,
    pub instance: String,
    /// In order: `rp` must sit in the first 400 bytes.
    pub txt: Vec<(String, String)>,
}

/// The printer, then one service per scanner.
pub fn services(advertised: &Advertised) -> Vec<Service> {
    let printer = Service {
        kind: ServiceType::printer(advertised.tls),
        instance: advertised.instance.clone(),
        txt: printer_txt(advertised),
    };
    let scanners = advertised.scanners.iter().map(|scanner| Service {
        kind: ServiceType::scanner(advertised.tls),
        instance: scanner.name.clone(),
        txt: scanner_txt(advertised, scanner),
    });
    std::iter::once(printer).chain(scanners).collect()
}

/// A TXT pair is one DNS string, at most 255 bytes with its `=`.
const TXT_PAIR_MAX: usize = 255;

/// The largest paper the queue offers is A3, up to 13 x 19 inches in Apple's table.
const PAPER_MAX: &str = "tabloid-A3";

/// The printer's TXT record, from Apple's Bonjour Printing and PWG 5100.14 section 4.2.
pub fn printer_txt(advertised: &Advertised) -> Vec<(String, String)> {
    let mut txt = vec![
        // Without the leading slash; Android drops a printer that has none.
        pair("rp", "ipp/print"),
        pair("txtvers", "1"),
        pair("qtotal", "1"),
        pair("note", &advertised.instance),
        // macOS and iOS then ask for a user name and a password (iOS over TLS only).
        pair("air", "username,password"),
    ];
    if advertised.tls {
        txt.push(pair("TLS", "1.2"));
    }
    txt.extend(page(&advertised.public_url, "jobs").map(|url| pair("adminurl", &url)));
    txt.extend([
        pair("UUID", &advertised.printer_uuid.to_string()),
        // recto's sane-airscan blacklist and its SANE filter recognise `recto*`.
        pair("ty", escl::MODEL),
        pair("product", &format!("({})", escl::MODEL)),
        pair("Color", "T"),
        pair("Duplex", "T"),
        pair("kind", "document"),
        pair("PaperMax", PAPER_MAX),
    ]);
    if !advertised.urf.is_empty() {
        // A 1setOf keyword in IPP, comma separated in TXT as ippeveprinter writes it.
        txt.push(pair("URF", &advertised.urf.join(",")));
    }
    let formats: Vec<&str> = advertised
        .formats
        .iter()
        .map(String::as_str)
        // PWG 5100.14: never in `pdl`, it means "detect", not a format.
        .filter(|format| *format != "application/octet-stream")
        .collect();
    txt.push(pair("pdl", &formats.join(",")));
    fit(txt)
}

/// A scanner's TXT record, as Mopria and sane-airscan read it.
pub fn scanner_txt(advertised: &Advertised, scanner: &AdvertisedScanner) -> Vec<(String, String)> {
    let mut txt = vec![
        pair("txtvers", "1"),
        pair("vers", escl::VERSION),
        // No leading or trailing slash, and never a personal address.
        pair("rs", &format!("eSCL/{}", scanner.key)),
        pair("ty", &model(&scanner.make_and_model)),
        pair("UUID", &scanner.uuid.to_string()),
        pair("note", &advertised.instance),
    ];
    txt.extend(page(&advertised.public_url, "scan").map(|url| pair("adminurl", &url)));
    txt.push(pair("pdl", "application/pdf,image/jpeg,image/png"));
    let modes = [
        (scanner.color, "color"),
        (scanner.grayscale, "grayscale"),
        (scanner.binary, "binary"),
    ];
    let sources = [(scanner.platen, "platen"), (scanner.adf, "adf")];
    for (key, offered) in [("cs", &modes[..]), ("is", &sources[..])] {
        let values: Vec<&str> = offered
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, value)| *value)
            .collect();
        if !values.is_empty() {
            txt.push(pair(key, &values.join(",")));
        }
    }
    txt.push(pair("duplex", if scanner.duplex { "T" } else { "F" }));
    fit(txt)
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

/// `recto <vendor> <model>`: the loop guard needs `ty` to start with `recto`.
fn model(make_and_model: &str) -> String {
    let make_and_model = make_and_model.trim();
    if make_and_model.is_empty() {
        escl::MODEL.to_owned()
    } else {
        format!("{} {make_and_model}", escl::MODEL)
    }
}

/// A page of recto's web interface, for `adminurl`.
fn page(public_url: &Url, path: &str) -> Option<String> {
    let mut base = public_url.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join(path).ok().map(String::from)
}

/// Pairs too long for a TXT string are left out rather than refused: a long public URL or
/// instance name must not stop the whole announcement. Values are cut at a character boundary.
fn fit(txt: Vec<(String, String)>) -> Vec<(String, String)> {
    txt.into_iter()
        .filter_map(|(key, value)| {
            let room = TXT_PAIR_MAX.checked_sub(key.len() + 1)?;
            match key.as_str() {
                "adminurl" if value.len() > room => None,
                _ if value.len() > room => {
                    let mut end = room;
                    while !value.is_char_boundary(end) {
                        end -= 1;
                    }
                    Some((key, value[..end].to_owned()))
                }
                _ => Some((key, value)),
            }
        })
        .collect()
}

/// The host name recto announces: `recto-<hostname>.local.`, lowercase, with every character
/// outside `a-z0-9-` replaced by `-`. Only the first label of the host name is kept.
pub fn advertised_host(hostname: &str) -> String {
    let label: String = hostname
        .split('.')
        .next()
        .unwrap_or_default()
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let label = label.trim_matches('-');
    let label = if label.is_empty() {
        escl::MODEL.to_owned()
    } else {
        format!("{}-{label}", escl::MODEL)
    };
    // A DNS label is at most 63 bytes, all ASCII here.
    let label = label[..label.len().min(63)].trim_end_matches('-');
    format!("{label}.local.")
}

/// The machine's host name. With host networking a container has the host's; on a bridge it
/// is the container id, which is why the Avahi files do not name a host.
pub fn system_hostname() -> io::Result<String> {
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))?;
    Ok(name.trim().to_owned())
}

/// A network interface with an address, as the system lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub loopback: bool,
    /// WireGuard, NetBird, Tailscale and tun devices are point-to-point.
    pub point_to_point: bool,
}

/// The interfaces that have an address, one entry per address.
pub fn candidates() -> io::Result<Vec<Candidate>> {
    Ok(if_addrs::get_if_addrs()?
        .into_iter()
        .map(|interface| Candidate {
            loopback: interface.is_loopback(),
            point_to_point: interface.is_p2p(),
            name: interface.name,
        })
        .collect())
}

/// Interfaces left out by default: containers, virtual machines and VPNs, whose addresses the
/// LAN cannot reach. Point-to-point interfaces are left out by flag as well.
const SKIPPED: &[&str] = &[
    "docker",
    "br-",
    "veth",
    "virbr",
    "vnet",
    "lxcbr",
    "lxdbr",
    "podman",
    "cni",
    "flannel",
    "cali",
    "tun",
    "tap",
    "wg",
    "wt",
    "tailscale",
    "zt",
    "nebula",
    "ham",
];

/// The interfaces to announce on: the chosen ones (`RECTO_DNSSD_INTERFACES`), each of which must
/// have an address, or with none chosen every interface but loopback, point-to-point and the
/// `SKIPPED` prefixes.
pub fn interfaces(chosen: &[String], available: &[Candidate]) -> Result<Vec<String>, DnssdError> {
    if let Some(unknown) = chosen
        .iter()
        .find(|name| !available.iter().any(|c| &c.name == *name))
    {
        return Err(DnssdError::UnknownInterface(unknown.clone()));
    }
    let names: BTreeSet<String> = if chosen.is_empty() {
        available
            .iter()
            .filter(|c| !c.loopback && !c.point_to_point)
            .filter(|c| !SKIPPED.iter().any(|prefix| c.name.starts_with(prefix)))
            .map(|c| c.name.clone())
            .collect()
    } else {
        chosen.iter().cloned().collect()
    };
    Ok(names.into_iter().collect())
}

/// Linux interface names are at most 15 bytes.
const INTERFACE_NAME_MAX: usize = 15;

/// Parses `RECTO_DNSSD_INTERFACES`: names separated by commas. Empty means the default
/// selection of [`interfaces`].
pub fn parse_interfaces(value: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for name in value.split(',').map(str::trim) {
        if name.is_empty() {
            continue;
        }
        if name.len() > INTERFACE_NAME_MAX
            || name
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '/')
        {
            return Err(format!("{name:?} is not an interface name"));
        }
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// Avahi `.service` files for the host's avahi-daemon, as `(file name, contents)`: one for the
/// printer and one per scanner. They name no host, so the records point at the host's own name,
/// where the published port is; Avahi publishes no address for a `<host-name>` it is given.
pub fn avahi_files(advertised: &Advertised) -> Vec<(String, String)> {
    services(advertised)
        .into_iter()
        .zip(
            std::iter::once("recto-printer.service".to_owned()).chain(
                advertised
                    .scanners
                    .iter()
                    .map(|scanner| format!("recto-scanner-{}.service", scanner.key)),
            ),
        )
        .map(|(service, file)| (file, avahi_service(&service, advertised.port)))
        .collect()
}

fn avahi_service(service: &Service, port: u16) -> String {
    let mut xml = String::from(concat!(
        "<?xml version=\"1.0\" standalone=\"no\"?>\n",
        "<!DOCTYPE service-group SYSTEM \"avahi-service.dtd\">\n",
        "<!-- Written by recto dnssd-services: copy to /etc/avahi/services/. -->\n",
        "<service-group>\n",
    ));
    let _ = writeln!(xml, "  <name>{}</name>", escape(&service.instance));
    // Docker publishes ports on IPv4 unless the network has IPv6 enabled.
    xml.push_str("  <service protocol=\"ipv4\">\n");
    let _ = writeln!(xml, "    <type>{}</type>", service.kind.name());
    for subtype in service.kind.subtypes() {
        let _ = writeln!(xml, "    <subtype>{subtype}</subtype>");
    }
    let _ = writeln!(xml, "    <port>{port}</port>");
    for (key, value) in &service.txt {
        let _ = writeln!(
            xml,
            "    <txt-record>{}={}</txt-record>",
            escape(key),
            escape(value)
        );
    }
    xml.push_str("  </service>\n</service-group>\n");
    xml
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Debug, thiserror::Error)]
pub enum DnssdError {
    #[error("RECTO_DNSSD_INTERFACES: no interface named {0}, or it has no address")]
    UnknownInterface(String),
    #[error("mDNS responder: {0}")]
    Mdns(#[from] mdns_sd::Error),
}

/// The full `mdns-sd` service: the type, with a printer's first subtype (`_universal`, which
/// Apple devices browse), under `host`. `mdns-sd` keeps one subtype per instance, so `_print`
/// is only in the Avahi files. The addresses follow the enabled interfaces.
pub fn service_info(service: &Service, host: &str, port: u16) -> Result<ServiceInfo, DnssdError> {
    let ty_domain = match service.kind.subtypes().first() {
        Some(subtype) => format!("{subtype}.local."),
        None => format!("{}.local.", service.kind.name()),
    };
    let info = ServiceInfo::new(
        &ty_domain,
        &service.instance,
        host,
        "",
        port,
        &service.txt[..],
    )?;
    Ok(info.enable_addr_auto())
}

/// Where services are registered: the `mdns-sd` daemon, or a recording fake in tests.
pub trait Registry {
    fn register(&self, service: ServiceInfo) -> Result<(), DnssdError>;
    /// Sends goodbye packets for one service.
    fn unregister(&self, fullname: &str) -> Result<(), DnssdError>;
    fn shutdown(self);
}

/// How long shutting down waits for the goodbye packets.
const GOODBYE_WAIT: Duration = Duration::from_secs(2);

/// The `mdns-sd` daemon: its own thread, sockets on 5353 shared with the host's Avahi.
pub struct MdnsRegistry {
    daemon: ServiceDaemon,
}

impl MdnsRegistry {
    /// Starts the daemon on `interfaces` only. `ipv6` false leaves AAAA records out, for an
    /// IPv4-only listener that IPv6 clients would find closed.
    pub fn start(interfaces: &[String], ipv6: bool) -> Result<Self, DnssdError> {
        let daemon = ServiceDaemon::new()?;
        // Selections apply in order and the last match wins: everything off, then the chosen.
        daemon.disable_interface(IfKind::All)?;
        let chosen: Vec<IfKind> = interfaces
            .iter()
            .map(|name| IfKind::Name(name.clone()))
            .collect();
        daemon.enable_interface(chosen)?;
        if !ipv6 {
            daemon.disable_interface(IfKind::IPv6)?;
        }
        Ok(Self { daemon })
    }
}

impl Registry for MdnsRegistry {
    fn register(&self, service: ServiceInfo) -> Result<(), DnssdError> {
        Ok(self.daemon.register(service)?)
    }

    fn unregister(&self, fullname: &str) -> Result<(), DnssdError> {
        let done = self.daemon.unregister(fullname)?;
        let _ = done.recv_timeout(GOODBYE_WAIT);
        Ok(())
    }

    fn shutdown(self) {
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(GOODBYE_WAIT);
        }
    }
}

/// The registered services, unregistered by [`Responder::shutdown`].
pub struct Responder<R: Registry> {
    registry: R,
    fullnames: Vec<String>,
}

/// Registers every service of `advertised`. A service that fails leaves the ones before it
/// registered, and they are unregistered before the error is returned.
pub fn start<R: Registry>(
    advertised: &Advertised,
    registry: R,
) -> Result<Responder<R>, DnssdError> {
    let mut responder = Responder {
        registry,
        fullnames: Vec::new(),
    };
    for service in services(advertised) {
        let registered =
            service_info(&service, &advertised.host, advertised.port).and_then(|info| {
                let fullname = info.get_fullname().to_owned();
                responder.registry.register(info).map(|()| fullname)
            });
        match registered {
            Ok(fullname) => responder.fullnames.push(fullname),
            Err(error) => {
                responder.shutdown();
                return Err(error);
            }
        }
    }
    Ok(responder)
}

impl<R: Registry> Responder<R> {
    /// The full names of the registered services.
    pub fn fullnames(&self) -> &[String] {
        &self.fullnames
    }

    /// Unregisters every service, then stops the registry. Blocks up to a few seconds per
    /// service for the goodbye packets: call it from a blocking task.
    pub fn shutdown(self) {
        for fullname in &self.fullnames {
            if let Err(error) = self.registry.unregister(fullname) {
                tracing::warn!(%error, service = %fullname, "could not unregister a DNS-SD service");
            }
        }
        self.registry.shutdown();
    }
}

#[cfg(test)]
mod tests;
