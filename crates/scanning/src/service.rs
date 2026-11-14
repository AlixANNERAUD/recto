use async_trait::async_trait;

use crate::{OutputFormat, ScanError, ScanSettings, ScannedPage, ScannerCapabilities, ScannerInfo};

/// What to scan, and how to keep it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRequest {
    pub settings: ScanSettings,
    /// How each page is compressed as soon as it is read (see [`ScannedPage`]). PNG and JPEG
    /// hold one image, so with them only the first page of a feeder batch is kept.
    pub format: OutputFormat,
    /// Stops after the first page, even from a feeder: a preview must never pull a whole stack
    /// through.
    pub single_page: bool,
}

/// What the server needs from a scanner stack. Implemented over libsane for real use and by a mock
/// for the tests of the consumers. Scans queue: only one runs at a time because a scanner has one
/// head.
#[async_trait]
pub trait ScannerService: Send + Sync + 'static {
    /// Lists scanners. The result is cached until `refresh` is true, since discovery can take
    /// seconds on network backends.
    async fn scanners(&self, refresh: bool) -> Result<Vec<ScannerInfo>, ScanError>;

    async fn capabilities(&self, id: &str) -> Result<ScannerCapabilities, ScanError>;

    /// One page for a flatbed or a single-page request, every page of the feeder otherwise.
    async fn scan(&self, id: &str, request: ScanRequest) -> Result<Vec<ScannedPage>, ScanError>;
}
