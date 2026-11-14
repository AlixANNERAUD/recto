use async_trait::async_trait;

use crate::{ScanError, ScanSettings, ScannedPage, ScannerCapabilities, ScannerInfo};

/// What the server needs from a scanner stack. Implemented over libsane for real use and by a mock
/// for the tests of the consumers. Scans queue: only one runs at a time because a scanner has one
/// head.
#[async_trait]
pub trait ScannerService: Send + Sync + 'static {
    /// Lists scanners. The result is cached until `refresh` is true, since discovery can take
    /// seconds on network backends.
    async fn scanners(&self, refresh: bool) -> Result<Vec<ScannerInfo>, ScanError>;

    async fn capabilities(&self, id: &str) -> Result<ScannerCapabilities, ScanError>;

    /// One page for a flatbed, every page of the feeder for a feeder source.
    async fn scan(&self, id: &str, settings: ScanSettings) -> Result<Vec<ScannedPage>, ScanError>;
}
