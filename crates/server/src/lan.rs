//! The LAN listener's TLS: the certificate devices that found recto through DNS-SD see.
//!
//! They connect by `.local` name, which the public certificate does not cover, so recto uses
//! the operator's certificate (`RECTO_LAN_TLS_CERTIFICATE` and `RECTO_LAN_TLS_KEY`) or a
//! self-signed one generated once and kept in `RECTO_DATA_DIR/lan-tls/`. Apple devices pin the
//! first certificate they see, so the generated one never changes from one start to the next.
//! Everything is on `ring`: the lockfile has no aws-lc-rs.
//!
//! The listener itself ([`TlsListener`], [`serve`]) runs each handshake in a task of its own,
//! under [`HANDSHAKE_TIMEOUT`]: a device that connects and says nothing holds only its own
//! connection, never the others'.

use std::{
    fmt::Write as _,
    fs::{self, DirBuilder, OpenOptions},
    future::Future,
    io::{self, Write as _},
    net::{IpAddr, SocketAddr},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{Router, serve::ListenerExt as _};
use chrono::{Datelike as _, Duration, Utc};
use rcgen::{
    CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, PKCS_ECDSA_P256_SHA256,
    date_time_ymd,
};
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
};
use sha2::{Digest as _, Sha256};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc},
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

use crate::dnssd::advertised_host;

/// The generated certificate's directory, under `RECTO_DATA_DIR`.
pub const DIRECTORY: &str = "lan-tls";
const CERTIFICATE_FILE: &str = "certificate.pem";
const KEY_FILE: &str = "key.pem";

/// How long the generated certificate lasts: devices pinned it, so it should outlive the
/// installation.
const VALIDITY_YEARS: i32 = 10;

/// The LAN listener's TLS configuration.
pub struct LanTls {
    pub config: Arc<ServerConfig>,
    /// The SHA-256 of the certificate, `AB:CD:...` as `openssl x509 -fingerprint` writes it,
    /// for the startup log: people compare it with what their device shows.
    pub fingerprint: String,
}

impl LanTls {
    /// Accepts TLS connections with this configuration.
    pub fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(self.config.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LanError {
    #[error("RECTO_LAN_TLS_CERTIFICATE and RECTO_LAN_TLS_KEY go together: set both or neither")]
    Pair,
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: no PEM certificate in this file", path.display())]
    NoCertificate { path: PathBuf },
    #[error("{}: {source}", path.display())]
    Pem {
        path: PathBuf,
        source: rustls::pki_types::pem::Error,
    },
    #[error("{}: other users can read this private key; make it readable by recto only (chmod 600)", path.display())]
    Permissions { path: PathBuf },
    #[error("generating the LAN certificate: {0}")]
    Generate(#[from] rcgen::Error),
    #[error("LAN TLS: {0}")]
    Tls(#[from] rustls::Error),
}

/// The names of the generated certificate: recto's advertised host name, the host's own
/// `.local` name (the Avahi files point there) and each specific address of the listener
/// (Android connects by address).
pub fn subject_names(hostname: &str, addresses: &[IpAddr]) -> Vec<String> {
    let mut names = vec![advertised_host(hostname).trim_end_matches('.').to_owned()];
    let label = hostname
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let valid = !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.len() <= 63
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        names.push(format!("{label}.local"));
    }
    names.extend(
        addresses
            .iter()
            .filter(|address| !address.is_unspecified())
            .map(IpAddr::to_string),
    );
    names.dedup();
    names
}

/// The operator's certificate and key when both are given, else the generated pair, created at
/// the first start for `hostname` and `addresses` and reused afterwards, whatever they become.
pub fn load_or_create(
    certificate: Option<&Path>,
    key: Option<&Path>,
    data_dir: &Path,
    hostname: &str,
    addresses: &[IpAddr],
) -> Result<LanTls, LanError> {
    match (certificate, key) {
        (Some(certificate), Some(key)) => load(certificate, key),
        (None, None) => {
            let directory = data_dir.join(DIRECTORY);
            let certificate = directory.join(CERTIFICATE_FILE);
            let key = directory.join(KEY_FILE);
            if !certificate.is_file() || !key.is_file() {
                generate(&directory, &subject_names(hostname, addresses))?;
            }
            check_private(&key)?;
            load(&certificate, &key)
        }
        _ => Err(LanError::Pair),
    }
}

fn load(certificate: &Path, key: &Path) -> Result<LanTls, LanError> {
    let chain = read(certificate)?;
    let chain = CertificateDer::pem_slice_iter(&chain)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| LanError::Pem {
            path: certificate.to_owned(),
            source,
        })?;
    let Some(leaf) = chain.first() else {
        return Err(LanError::NoCertificate {
            path: certificate.to_owned(),
        });
    };
    let fingerprint = fingerprint(leaf);
    let key = PrivateKeyDer::from_pem_slice(&read(key)?).map_err(|source| LanError::Pem {
        path: key.to_owned(),
        source,
    })?;
    Ok(LanTls {
        config: Arc::new(server_config(chain, key)?),
        fingerprint,
    })
}

/// A TLS 1.2 and 1.3 server configuration on `ring`, without client certificates.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<ServerConfig, LanError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    // Also checks that the key belongs to the certificate.
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
    // IPP and eSCL clients speak HTTP/1.1.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

fn read(path: &Path) -> Result<Vec<u8>, LanError> {
    fs::read(path).map_err(|source| LanError::Io {
        path: path.to_owned(),
        source,
    })
}

/// The generated key must stay recto's: a backup or a copy that widened its mode would let
/// anyone on the host impersonate the printer to devices that pinned it.
fn check_private(key: &Path) -> Result<(), LanError> {
    let mode = fs::metadata(key)
        .map_err(|source| LanError::Io {
            path: key.to_owned(),
            source,
        })?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(LanError::Permissions {
            path: key.to_owned(),
        });
    }
    Ok(())
}

/// An ECDSA P-256 self-signed certificate for `names`, the first one as its common name.
fn generate(directory: &Path, names: &[String]) -> Result<(), LanError> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
    let mut params = CertificateParams::new(names.to_vec())?;
    params
        .distinguished_name
        .push(DnType::CommonName, names[0].clone());
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    // A day early, for devices whose clock is behind.
    let start = (Utc::now() - Duration::days(1)).date_naive();
    // Month and day come from a valid date, so they fit in a u8.
    params.not_before = date_time_ymd(start.year(), start.month() as u8, start.day() as u8);
    // The 28th at most, so that the 29th of February has a match ten years later.
    params.not_after = date_time_ymd(
        start.year() + VALIDITY_YEARS,
        start.month() as u8,
        start.day().min(28) as u8,
    );
    let certificate = params.self_signed(&key)?;

    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|source| LanError::Io {
            path: directory.to_owned(),
            source,
        })?;
    // The key first: the certificate's presence is what marks the pair complete.
    write_private(&directory.join(KEY_FILE), &key.serialize_pem())?;
    write_private(&directory.join(CERTIFICATE_FILE), &certificate.pem())
}

/// Written to a temporary file with mode 0600 and renamed, so a crash never leaves half a key.
fn write_private(path: &Path, contents: &str) -> Result<(), LanError> {
    let temporary = path.with_extension("pem.tmp");
    let error = |source| LanError::Io {
        path: path.to_owned(),
        source,
    };
    // A leftover from a crash may have another mode, which `mode` below would not change.
    match fs::remove_file(&temporary) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(LanError::Io {
                path: temporary,
                source: error,
            });
        }
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(error)?;
    file.write_all(contents.as_bytes()).map_err(error)?;
    file.sync_all().map_err(error)?;
    fs::rename(&temporary, path).map_err(error)
}

/// The SHA-256 of a DER certificate, as uppercase hex pairs separated by colons.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .fold(String::with_capacity(95), |mut text, byte| {
            if !text.is_empty() {
                text.push(':');
            }
            let _ = write!(text, "{byte:02X}");
            text
        })
}

/// How long a device has to complete its TLS handshake.
pub const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How many handshakes may run at once; past that, connections wait in the kernel's backlog.
const HANDSHAKES: usize = 128;

/// TLS connections, ready for HTTP: an axum listener over a TCP listener, whose handshakes run
/// in tasks of their own and reach [`accept`](axum::serve::Listener::accept) once complete.
pub struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    /// Accepts on `tcp` in the background until the listener is dropped. A handshake that fails
    /// or outlasts `handshake_timeout` closes its connection.
    pub fn new(
        tcp: TcpListener,
        acceptor: TlsAcceptor,
        handshake_timeout: std::time::Duration,
    ) -> Self {
        // Reading the address of a bound socket does not fail on Linux.
        let local = tcp
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let (sender, ready) = mpsc::channel(HANDSHAKES);
        tokio::spawn(accept_connections(tcp, acceptor, handshake_timeout, sender));
        Self { ready, local }
    }
}

async fn accept_connections(
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    handshake_timeout: std::time::Duration,
    ready: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    let handshakes = Arc::new(Semaphore::new(HANDSHAKES));
    loop {
        let permit = tokio::select! {
            permit = Arc::clone(&handshakes).acquire_owned() => permit,
            () = ready.closed() => return,
        };
        // The semaphore is never closed.
        let Ok(permit) = permit else { return };
        let (stream, peer) = tokio::select! {
            accepted = tcp.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Out of file descriptors, most likely: wait for some to close.
                    tracing::warn!(%error, "the LAN listener could not accept a connection");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            },
            () = ready.closed() => return,
        };
        let acceptor = acceptor.clone();
        let ready = ready.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
                Ok(Ok(stream)) => {
                    let _ = ready.send((stream, peer)).await;
                }
                // Scanners on the LAN probe ports and give up halfway: not worth a warning.
                Ok(Err(error)) => tracing::debug!(%error, %peer, "LAN TLS handshake failed"),
                Err(_) => tracing::debug!(%peer, "LAN TLS handshake timed out"),
            }
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            // The accepting task stops only when this listener is dropped.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Serves `router` on `listener` until `shutdown` resolves, then lets requests in flight finish.
/// Routes see the TCP peer as `ConnectInfo<SocketAddr>`, as on the main listener.
pub async fn serve(
    listener: TlsListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    // axum hands the peer address to custom listeners only through `tap_io`; it also turns
    // Nagle off, as small IPP and eSCL answers would otherwise wait for an acknowledgement.
    let listener = listener.tap_io(|stream| {
        let _ = stream.get_ref().0.set_nodelay(true);
    });
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}

#[cfg(test)]
mod tests;
