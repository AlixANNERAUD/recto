use async_trait::async_trait;

use crate::{OutputFormat, ScanError, ScanSettings, ScannedPage, ScannerCapabilities, ScannerInfo};

/// What to scan, and how to keep it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanRequest {
    pub settings: ScanSettings,
    /// How each page is compressed as soon as it is read (see [`ScannedPage`]). PNG and JPEG
    /// hold one image, so with them [`ScannerService::scan`] keeps only the first page of a
    /// feeder batch; [`ScannerService::scan_every_page`] keeps them all.
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

    /// One page for a flatbed or a single-page request, every page of the feeder otherwise, but
    /// only the first one in PNG or JPEG: the scan page stores a single file, and one image
    /// holds one page.
    async fn scan(&self, id: &str, request: ScanRequest) -> Result<Vec<ScannedPage>, ScanError>;

    /// As [`scan`](Self::scan), but keeps every page of a feeder batch in PNG and JPEG too: eSCL
    /// hands such pages out one document each, and sane-airscan only ever asks for images.
    ///
    /// The default is `scan`, right for a service that keeps every page anyway or cannot scan
    /// at all. One that drops pages in `scan` must override it.
    async fn scan_every_page(
        &self,
        id: &str,
        request: ScanRequest,
    ) -> Result<Vec<ScannedPage>, ScanError> {
        self.scan(id, request).await
    }

    /// The DNS-SD instance names recto announces its own scanners under, which a network
    /// backend would list them by if it found them again: [`scanners`](Self::scanners) leaves
    /// such devices out from then on. Replaces the previous names. The default ignores them,
    /// right for a stack without network backends.
    fn hide_advertised(&self, _names: Vec<String>) {}
}
