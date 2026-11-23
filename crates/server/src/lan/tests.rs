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

/// A TLS connection to `address` from `from`, trusting `trusted`.
async fn connect_tls(
    address: std::net::SocketAddr,
    trusted: &[u8],
    from: IpAddr,
) -> tokio_rustls::client::TlsStream<tokio::net::TcpStream> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(trusted.to_vec())).unwrap();
    let client =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
    let tcp = connect_from(address, from).await;
    let name = ServerName::try_from("127.0.0.1").unwrap();
    TlsConnector::from(Arc::new(client))
        .connect(name, tcp)
        .await
        .unwrap()
}

/// A TCP connection to `address` from `from`: any address of 127.0.0.0/8 is loopback on Linux,
/// which makes a second client.
async fn connect_from(address: std::net::SocketAddr, from: IpAddr) -> tokio::net::TcpStream {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind(std::net::SocketAddr::new(from, 0)).unwrap();
    socket.connect(address).await.unwrap()
}

/// One HTTP request over TLS to `address`, trusting `trusted`; the whole answer.
async fn get_over_tls(address: std::net::SocketAddr, trusted: &[u8]) -> String {
    get_over_tls_from(address, trusted, LOOPBACK).await
}

async fn get_over_tls_from(address: std::net::SocketAddr, trusted: &[u8], from: IpAddr) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = connect_tls(address, trusted, from).await;
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    String::from_utf8(answer).unwrap()
}

/// Whether `stream` is closed within 5 seconds, without a byte.
async fn closed_soon(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> bool {
    use tokio::io::AsyncReadExt as _;
    let mut byte = [0; 1];
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut byte)).await,
        Ok(Ok(0) | Err(_))
    )
}

/// A listener on loopback answering `/` with the peer's address, and its certificate.
async fn listening(
    timeout: std::time::Duration,
) -> (
    std::net::SocketAddr,
    Vec<u8>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<io::Result<()>>,
    tempfile::TempDir,
) {
    use axum::{Router, extract::ConnectInfo, routing::get};
    let data = recto_domain::testing::temporary_directory();
    let tls = load_or_create(None, None, data.path(), "nas", &[LOOPBACK]).unwrap();
    let der = certificate_der(&data.path().join(DIRECTORY).join("certificate.pem"));
    let tcp = tokio::net::TcpListener::bind((LOOPBACK, 0)).await.unwrap();
    let address = tcp.local_addr().unwrap();
    let router = Router::new().route(
        "/",
        get(
            |ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>| async move {
                format!("peer {}", peer.ip())
            },
        ),
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(
        TlsListener::new(tcp, tls.acceptor(), timeout),
        router,
        async {
            let _ = stopped.await;
        },
    ));
    (address, der, stop, server, data)
}

#[tokio::test]
async fn a_stalled_handshake_does_not_block_others_and_times_out() {
    use tokio::io::AsyncReadExt as _;
    let (address, der, stop, server, _data) =
        listening(std::time::Duration::from_millis(300)).await;
    // A client that connects and never says anything.
    let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        get_over_tls(address, &der),
    )
    .await
    .expect("another client is served meanwhile");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    // The routes see the TCP peer, as on the main listener.
    assert!(answer.ends_with("peer 127.0.0.1"), "{answer}");
    // The stalled one is dropped once its handshake time is up.
    let mut byte = [0; 1];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), stalled.read(&mut byte))
        .await
        .expect("the stalled connection is closed");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");

    let _ = stop.send(());
    tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("the listener stops")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn plain_http_is_never_answered() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let (address, _der, _stop, _server, _data) = listening(HANDSHAKE_TIMEOUT).await;
    let mut plain = tokio::net::TcpStream::connect(address).await.unwrap();
    plain
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        plain.read_to_end(&mut answer),
    )
    .await
    .expect("the connection is closed");
    assert!(!answer.starts_with(b"HTTP/"), "{answer:?}");
}

const SECOND_CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

#[tokio::test]
async fn one_address_runs_eight_handshakes_at_once() {
    let (address, der, _stop, _server, _data) = listening(std::time::Duration::from_secs(60)).await;
    // Eight connections that never start their handshake hold all of their address's places.
    let mut stalled = Vec::new();
    for _ in 0..HANDSHAKES_PER_ADDRESS {
        stalled.push(connect_from(address, LOOPBACK).await);
    }
    let mut ninth = connect_from(address, LOOPBACK).await;
    assert!(
        closed_soon(&mut ninth).await,
        "a ninth handshake is refused"
    );
    // Another device is served meanwhile.
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        get_over_tls_from(address, &der, SECOND_CLIENT),
    )
    .await
    .expect("another address is served");
    assert!(answer.ends_with("peer 127.0.0.2"), "{answer}");
    // Once one gives up, its place is free again.
    drop(stalled.pop());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        get_over_tls(address, &der),
    )
    .await
    .expect("a place is free");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

#[tokio::test]
async fn one_address_keeps_thirty_two_connections() {
    let (address, der, _stop, _server, _data) = listening(HANDSHAKE_TIMEOUT).await;
    let mut open = Vec::new();
    for _ in 0..CONNECTIONS_PER_ADDRESS {
        open.push(connect_tls(address, &der, LOOPBACK).await);
    }
    let mut over = connect_from(address, LOOPBACK).await;
    assert!(
        closed_soon(&mut over).await,
        "one connection too many is refused"
    );
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        get_over_tls_from(address, &der, SECOND_CLIENT),
    )
    .await
    .expect("another address is served");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    drop(open.pop());
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        get_over_tls(address, &der),
    )
    .await
    .expect("a place is free");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}
