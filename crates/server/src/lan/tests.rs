use std::net::{IpAddr, Ipv4Addr};

use rustls::{ClientConfig, RootCertStore, pki_types::ServerName};
use tokio_rustls::TlsConnector;

use super::*;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Whether a client trusting only `trusted` completes a handshake with `tls` for `name`.
async fn handshake(tls: &LanTls, trusted: &[u8], name: &str) -> bool {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(trusted.to_vec())).unwrap();
    let client =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let acceptor = tls.acceptor();
    let connector = TlsConnector::from(Arc::new(client));
    let name = ServerName::try_from(name.to_owned()).unwrap();
    let (server, client) = tokio::join!(
        acceptor.accept(server_io),
        connector.connect(name, client_io)
    );
    server.is_ok() && client.is_ok()
}

fn certificate_der(path: &Path) -> Vec<u8> {
    let pem = std::fs::read(path).unwrap();
    CertificateDer::from_pem_slice(&pem).unwrap().to_vec()
}

#[tokio::test]
async fn the_generated_certificate_is_kept_and_names_the_advertised_host() {
    let data = tempfile::tempdir().unwrap();
    let first = load_or_create(None, None, data.path(), "Nas-1", &[LOOPBACK]).unwrap();
    let second = load_or_create(None, None, data.path(), "Nas-1", &[LOOPBACK]).unwrap();
    assert_eq!(first.fingerprint, second.fingerprint);

    let directory = data.path().join(DIRECTORY);
    let certificate = directory.join("certificate.pem");
    assert_eq!(mode(&directory), 0o700);
    assert_eq!(mode(&certificate), 0o600);
    assert_eq!(mode(&directory.join("key.pem")), 0o600);

    let der = certificate_der(&certificate);
    assert_eq!(first.fingerprint, fingerprint(&der));
    for name in ["recto-nas-1.local", "nas-1.local", "127.0.0.1"] {
        assert!(handshake(&second, &der, name).await, "{name}");
    }
    assert!(!handshake(&second, &der, "other.local").await);
}

#[test]
fn the_names_are_recto_s_own_the_host_s_and_the_addresses() {
    assert_eq!(
        subject_names(
            "nas.home.arpa",
            &[LOOPBACK, IpAddr::V4(Ipv4Addr::UNSPECIFIED)]
        ),
        ["recto-nas.local", "nas.local", "127.0.0.1"]
    );
    assert_eq!(subject_names("_", &[]), ["recto.local"]);
}

#[test]
fn a_generated_key_others_can_read_is_refused() {
    let data = tempfile::tempdir().unwrap();
    load_or_create(None, None, data.path(), "nas", &[]).unwrap();
    let key = data.path().join(DIRECTORY).join("key.pem");
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        load_or_create(None, None, data.path(), "nas", &[]),
        Err(LanError::Permissions { path }) if path == key
    ));
}

#[test]
fn certificate_and_key_go_together() {
    let data = tempfile::tempdir().unwrap();
    let file = data.path().join("certificate.pem");
    assert!(matches!(
        load_or_create(Some(&file), None, data.path(), "nas", &[]),
        Err(LanError::Pair)
    ));
    assert!(matches!(
        load_or_create(None, Some(&file), data.path(), "nas", &[]),
        Err(LanError::Pair)
    ));
}

fn write_pair(dir: &Path, name: &str, key: &rcgen::KeyPair) -> (PathBuf, PathBuf) {
    let certificate = rcgen::CertificateParams::new(vec![name.to_owned()])
        .unwrap()
        .self_signed(key)
        .unwrap();
    let certificate_path = dir.join("operator.pem");
    let key_path = dir.join("operator.key");
    std::fs::write(&certificate_path, certificate.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    (certificate_path, key_path)
}

#[tokio::test]
async fn the_operator_s_files_are_used_as_they_are() {
    let data = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let (certificate, key) = write_pair(data.path(), "printer.example.org", &key);
    // A key mounted from a secret is often readable by every user of the container.
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let tls = load_or_create(Some(&certificate), Some(&key), data.path(), "nas", &[]).unwrap();
    let der = certificate_der(&certificate);
    assert_eq!(tls.fingerprint, fingerprint(&der));
    assert!(!data.path().join(DIRECTORY).exists());
    assert!(handshake(&tls, &der, "printer.example.org").await);
}

#[test]
fn a_key_that_does_not_match_or_a_file_without_pem_is_refused() {
    let data = tempfile::tempdir().unwrap();
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let (certificate, _) = write_pair(data.path(), "printer.example.org", &key);
    let other = data.path().join("other.key");
    let other_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    std::fs::write(&other, other_key.serialize_pem()).unwrap();
    assert!(matches!(
        load_or_create(Some(&certificate), Some(&other), data.path(), "nas", &[]),
        Err(LanError::Tls(_))
    ));

    let garbage = data.path().join("garbage.pem");
    std::fs::write(&garbage, "not a certificate").unwrap();
    assert!(matches!(
        load_or_create(Some(&garbage), Some(&other), data.path(), "nas", &[]),
        Err(LanError::NoCertificate { path }) if path == garbage
    ));
    assert!(matches!(
        load_or_create(Some(&certificate), Some(&garbage), data.path(), "nas", &[]),
        Err(LanError::Pem { path, .. }) if path == garbage
    ));
    let missing = data.path().join("missing.pem");
    assert!(matches!(
        load_or_create(Some(&missing), Some(&other), data.path(), "nas", &[]),
        Err(LanError::Io { path, .. }) if path == missing
    ));
}

#[test]
fn the_fingerprint_is_written_like_openssl_s() {
    let fingerprint = fingerprint(b"recto");
    assert_eq!(fingerprint.len(), 32 * 3 - 1);
    assert!(fingerprint.starts_with("FA:1C:49:"), "{fingerprint}");
    assert!(fingerprint.split(':').all(|byte| {
        byte.len() == 2
            && byte
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
    }));
}
