//! The LAN listener's TLS: the certificate devices that found recto through DNS-SD see.
//!
//! They connect by `.local` name, which the public certificate does not cover, so recto uses
//! the operator's certificate (`RECTO_LAN_TLS_CERTIFICATE` and `RECTO_LAN_TLS_KEY`) or a
//! self-signed one generated once and kept in `RECTO_DATA_DIR/lan-tls/`. Apple devices pin the
//! first certificate they see, so the generated one never changes from one start to the next.
//! Everything is on `ring`: the lockfile has no aws-lc-rs.

use std::{
    fmt::Write as _,
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Write as _},
    net::IpAddr,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::Arc,
};

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
use tokio_rustls::TlsAcceptor;

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

#[cfg(test)]
mod tests;
