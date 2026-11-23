use std::{cell::RefCell, rc::Rc};

use super::*;

fn advertised() -> Advertised {
    Advertised {
        instance: "recto".into(),
        host: advertised_host("nas"),
        port: 8631,
        tls: true,
        public_url: Url::parse("https://recto.example.org").unwrap(),
        printer_uuid: Uuid::parse_str("7d3f0b6e-2c1a-5e9d-8f41-0a6b3c2d1e5f").unwrap(),
        formats: [
            "application/pdf",
            "image/jpeg",
            "image/png",
            "application/octet-stream",
            "image/pwg-raster",
            "image/urf",
        ]
        .map(String::from)
        .to_vec(),
        urf: ["V1.4", "CP1", "W8", "SRGB24", "RS300", "DM1", "IS1"]
            .map(String::from)
            .to_vec(),
        scanners: vec![AdvertisedScanner {
            key: "1a2b3c4d".into(),
            name: "Office scanner".into(),
            uuid: Uuid::parse_str("0f3c1d5e-8a43-5b52-9d0e-3b6c1f2a4e7d").unwrap(),
            make_and_model: "Canon LiDE 300".into(),
            color: true,
            grayscale: true,
            binary: false,
            platen: true,
            adf: true,
            duplex: true,
        }],
    }
}

fn value<'a>(txt: &'a [(String, String)], key: &str) -> Option<&'a str> {
    txt.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

#[test]
fn printer_txt_matches_airprint() {
    let txt = printer_txt(&advertised());
    let keys: Vec<&str> = txt.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "rp", "txtvers", "qtotal", "note", "air", "TLS", "adminurl", "UUID", "ty", "product",
            "Color", "Duplex", "kind", "PaperMax", "URF", "pdl"
        ]
    );
    assert_eq!(value(&txt, "rp"), Some("ipp/print"));
    assert_eq!(value(&txt, "air"), Some("username,password"));
    assert_eq!(value(&txt, "TLS"), Some("1.2"));
    assert_eq!(
        value(&txt, "adminurl"),
        Some("https://recto.example.org/jobs")
    );
    assert_eq!(
        value(&txt, "UUID"),
        Some("7d3f0b6e-2c1a-5e9d-8f41-0a6b3c2d1e5f")
    );
    assert_eq!(value(&txt, "ty"), Some("recto"));
    assert_eq!(value(&txt, "product"), Some("(recto)"));
    assert_eq!(value(&txt, "PaperMax"), Some("tabloid-A3"));
    assert_eq!(value(&txt, "URF"), Some("V1.4,CP1,W8,SRGB24,RS300,DM1,IS1"));
    assert_eq!(
        value(&txt, "pdl"),
        Some("application/pdf,image/jpeg,image/png,image/pwg-raster,image/urf")
    );
    let total: usize = txt.iter().map(|(k, v)| k.len() + v.len() + 2).sum();
    assert!(txt.iter().all(|(k, v)| k.len() + v.len() < 255));
    assert!(total < 1300, "{total} bytes");
    // `rp` within the first 400 bytes (PWG 5100.14).
    assert!(txt[0].0 == "rp");
}

#[test]
fn a_queue_without_apple_raster_announces_no_urf_and_plain_ipp_no_tls() {
    let mut advertised = advertised();
    advertised.urf.clear();
    advertised.tls = false;
    let txt = printer_txt(&advertised);
    assert_eq!(value(&txt, "URF"), None);
    assert_eq!(value(&txt, "TLS"), None);
    assert_eq!(services(&advertised)[0].kind, ServiceType::Ipp);
    assert_eq!(services(&advertised)[1].kind, ServiceType::Uscan);
}

#[test]
fn a_public_url_under_a_path_keeps_it_and_a_huge_one_is_left_out() {
    let mut advertised = advertised();
    advertised.public_url = Url::parse("https://example.org/recto").unwrap();
    let txt = printer_txt(&advertised);
    assert_eq!(
        value(&txt, "adminurl"),
        Some("https://example.org/recto/jobs")
    );
    advertised.public_url =
        Url::parse(&format!("https://example.org/{}", "a".repeat(300))).unwrap();
    assert_eq!(value(&printer_txt(&advertised), "adminurl"), None);
    advertised.instance = "é".repeat(200);
    let note = value(&printer_txt(&advertised), "note").unwrap().to_owned();
    assert!(note.len() <= 250 && note.chars().all(|c| c == 'é'));
}

#[test]
fn every_announced_model_starts_with_recto() {
    let advertised = advertised();
    let txt = scanner_txt(&advertised, &advertised.scanners[0]);
    assert_eq!(
        txt,
        [
            ("txtvers", "1"),
            ("vers", "2.63"),
            ("rs", "eSCL/1a2b3c4d"),
            ("ty", "recto Canon LiDE 300"),
            ("UUID", "0f3c1d5e-8a43-5b52-9d0e-3b6c1f2a4e7d"),
            ("note", "recto"),
            ("adminurl", "https://recto.example.org/scan"),
            ("pdl", "application/pdf,image/jpeg,image/png"),
            ("cs", "color,grayscale"),
            ("is", "platen,adf"),
            ("duplex", "T"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
    );
    let mut unnamed = advertised.scanners[0].clone();
    unnamed.make_and_model = " ".into();
    assert_eq!(
        value(&scanner_txt(&advertised, &unnamed), "ty"),
        Some("recto")
    );
    for service in services(&advertised) {
        assert!(value(&service.txt, "ty").unwrap().starts_with(escl::MODEL));
    }
}

#[test]
fn the_four_service_types_and_their_subtypes() {
    assert_eq!(ServiceType::printer(true).name(), "_ipps._tcp");
    assert_eq!(ServiceType::printer(false).name(), "_ipp._tcp");
    assert_eq!(ServiceType::scanner(true).name(), "_uscans._tcp");
    assert_eq!(ServiceType::scanner(false).name(), "_uscan._tcp");
    assert!(ServiceType::Uscans.tls() && !ServiceType::Ipp.tls());
    assert_eq!(
        ServiceType::Ipps.subtypes(),
        ["_universal._sub._ipps._tcp", "_print._sub._ipps._tcp"]
    );
    assert_eq!(
        ServiceType::Ipp.subtypes(),
        ["_universal._sub._ipp._tcp", "_print._sub._ipp._tcp"]
    );
    assert!(ServiceType::Uscans.subtypes().is_empty());
}

#[test]
fn the_advertised_host_is_never_the_hosts_own() {
    assert_eq!(advertised_host("NAS_1"), "recto-nas-1.local.");
    assert_eq!(advertised_host("nas.home.arpa"), "recto-nas.local.");
    assert_eq!(advertised_host("__"), "recto.local.");
    assert_eq!(advertised_host(""), "recto.local.");
    let long = advertised_host(&"x".repeat(100));
    assert_eq!(long.len(), 63 + ".local.".len());
    assert_eq!(
        advertised_host(&format!("{}-y", "x".repeat(56))).len(),
        62 + 7
    );
}

fn candidate(name: &str, loopback: bool, point_to_point: bool) -> Candidate {
    Candidate {
        name: name.into(),
        loopback,
        point_to_point,
    }
}

#[test]
fn docker_and_vpn_interfaces_are_skipped_by_default() {
    let available = [
        candidate("lo", true, false),
        candidate("eth0", false, false),
        candidate("eth0", false, false),
        candidate("docker0", false, false),
        candidate("docker_gwbridge", false, false),
        candidate("br-ab12", false, false),
        candidate("veth1", false, false),
        candidate("wg0", false, true),
        candidate("tailscale0", false, false),
        candidate("wt0", false, false),
        candidate("vpn0", false, true),
        candidate("wlan0", false, false),
    ];
    assert_eq!(interfaces(&[], &available).unwrap(), ["eth0", "wlan0"]);
    assert_eq!(interfaces(&["wg0".into()], &available).unwrap(), ["wg0"]);
    assert!(matches!(
        interfaces(&["eth9".into()], &available),
        Err(DnssdError::UnknownInterface(name)) if name == "eth9"
    ));
}

#[test]
fn the_interfaces_setting_is_a_comma_separated_list() {
    assert_eq!(parse_interfaces("").unwrap(), Vec::<String>::new());
    assert_eq!(
        parse_interfaces(" eth0, wlan0 ,,eth0").unwrap(),
        ["eth0", "wlan0"]
    );
    assert!(parse_interfaces("eth 0").is_err());
    assert!(parse_interfaces("a/b").is_err());
    assert!(parse_interfaces(&"x".repeat(16)).is_err());
}

#[test]
fn avahi_files_are_valid_service_groups() {
    let mut advertised = advertised();
    advertised.instance = "Print & <scan>".into();
    let files = avahi_files(&advertised);
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["recto-printer.service", "recto-scanner-1a2b3c4d.service"]
    );
    let printer = &files[0].1;
    assert!(printer.starts_with("<?xml version=\"1.0\" standalone=\"no\"?>\n"));
    assert!(printer.contains("<!DOCTYPE service-group SYSTEM \"avahi-service.dtd\">"));
    assert!(printer.contains("<name>Print &amp; &lt;scan&gt;</name>"));
    assert!(printer.contains("<type>_ipps._tcp</type>"));
    assert!(printer.contains("<subtype>_universal._sub._ipps._tcp</subtype>"));
    assert!(printer.contains("<subtype>_print._sub._ipps._tcp</subtype>"));
    assert!(printer.contains("<port>8631</port>"));
    assert!(printer.contains("<txt-record>note=Print &amp; &lt;scan&gt;</txt-record>"));
    assert!(!printer.contains("host-name"));
    assert_eq!(
        printer.matches("<txt-record>").count(),
        printer_txt(&advertised).len()
    );
    let scanner = &files[1].1;
    assert!(scanner.contains("<name>Office scanner</name>"));
    assert!(scanner.contains("<type>_uscans._tcp</type>"));
    assert!(!scanner.contains("<subtype>"));
    assert!(scanner.contains("<txt-record>rs=eSCL/1a2b3c4d</txt-record>"));
}

#[test]
fn the_example_avahi_file_is_what_recto_writes() {
    let mut advertised = advertised();
    advertised.scanners.clear();
    let files = avahi_files(&advertised);
    assert_eq!(
        files[0].1,
        include_str!("../../../../examples/avahi/recto-printer.service")
    );
}

#[test]
fn subtypes_are_registered() {
    let advertised = advertised();
    let services = services(&advertised);
    let printer = service_info(&services[0], &advertised.host, advertised.port).unwrap();
    assert_eq!(printer.get_type(), "_ipps._tcp.local.");
    assert_eq!(
        printer.get_subtype().as_deref(),
        Some("_universal._sub._ipps._tcp.local.")
    );
    assert_eq!(printer.get_hostname(), "recto-nas.local.");
    assert_eq!(printer.get_port(), 8631);
    assert!(printer.is_addr_auto());
    assert_eq!(printer.get_property_val_str("rp"), Some("ipp/print"));
    let scanner = service_info(&services[1], &advertised.host, advertised.port).unwrap();
    assert_eq!(scanner.get_type(), "_uscans._tcp.local.");
    assert_eq!(scanner.get_subtype(), &None);
    assert_eq!(scanner.get_fullname(), "Office scanner._uscans._tcp.local.");
}

#[derive(Clone, Default)]
struct Recorder {
    calls: Rc<RefCell<Vec<String>>>,
    refuse: Option<&'static str>,
}

impl Registry for Recorder {
    fn register(&self, service: ServiceInfo) -> Result<(), DnssdError> {
        if self.refuse == Some(service.get_type()) {
            return Err(DnssdError::Mdns(mdns_sd::Error::Again));
        }
        self.calls
            .borrow_mut()
            .push(format!("register {}", service.get_fullname()));
        Ok(())
    }

    fn unregister(&self, fullname: &str) -> Result<(), DnssdError> {
        self.calls
            .borrow_mut()
            .push(format!("unregister {fullname}"));
        Ok(())
    }

    fn shutdown(self) {
        self.calls.borrow_mut().push("shutdown".into());
    }
}

#[test]
fn the_responder_registers_every_service_and_unregisters_them() {
    let recorder = Recorder::default();
    let calls = recorder.calls.clone();
    let responder = start(&advertised(), recorder).unwrap();
    assert_eq!(
        responder.fullnames(),
        [
            "recto._ipps._tcp.local.",
            "Office scanner._uscans._tcp.local."
        ]
    );
    responder.shutdown();
    assert_eq!(
        *calls.borrow(),
        [
            "register recto._ipps._tcp.local.",
            "register Office scanner._uscans._tcp.local.",
            "unregister recto._ipps._tcp.local.",
            "unregister Office scanner._uscans._tcp.local.",
            "shutdown",
        ]
    );
}

#[test]
fn a_failed_registration_withdraws_the_others() {
    let recorder = Recorder {
        refuse: Some("_uscans._tcp.local."),
        ..Recorder::default()
    };
    let calls = recorder.calls.clone();
    assert!(start(&advertised(), recorder).is_err());
    assert_eq!(
        *calls.borrow(),
        [
            "register recto._ipps._tcp.local.",
            "unregister recto._ipps._tcp.local.",
            "shutdown",
        ]
    );
}

#[test]
fn no_aws_lc_in_the_lockfile() {
    assert!(!include_str!("../../../../Cargo.lock").contains("aws-lc"));
}
