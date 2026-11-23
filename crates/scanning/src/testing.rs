use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use image::{DynamicImage, Rgb, RgbImage};

use crate::{
    Area, OutputFormat, ScanError, ScanMode, ScanRequest, ScannedPage, ScannerCapabilities,
    ScannerInfo, ScannerService, Source,
};

/// A scanner stack for tests of the server and the interface: fixed scanners, a generated sample
/// document, optional failure and delay, and a record of what was asked.
pub struct MockScannerService {
    scanners: Vec<ScannerInfo>,
    capabilities: ScannerCapabilities,
    failure: Mutex<Option<ScanError>>,
    delay: Duration,
    /// How many sheets a feeder scan reads.
    feeder_pages: usize,
    /// Scans queue behind this lock like they do behind the SANE worker.
    queue: tokio::sync::Mutex<()>,
    active: AtomicUsize,
    peak: AtomicUsize,
    /// Counts the refreshed listings, for [`ScannerService::listing`].
    listings: AtomicU64,
    requests: Mutex<Vec<(String, ScanRequest)>>,
    /// Every id a caller tried to open, listed or not.
    opened: Mutex<Vec<String>>,
    /// The names [`ScannerService::hide_advertised`] was last given.
    hidden: Mutex<Vec<String>>,
}

impl Default for MockScannerService {
    fn default() -> Self {
        Self {
            scanners: vec![ScannerInfo {
                id: "mock:0".into(),
                vendor: "Recto".into(),
                model: "Mock scanner".into(),
                kind: "flatbed scanner".into(),
            }],
            capabilities: ScannerCapabilities {
                resolutions: vec![75, 100, 150, 200, 300, 600],
                modes: vec![ScanMode::Color, ScanMode::Gray, ScanMode::Lineart],
                sources: vec![Source::new("Flatbed"), Source::new("ADF")],
                area: Some(Area {
                    left: 0.0,
                    top: 0.0,
                    right: 215.9,
                    bottom: 297.0,
                }),
            },
            failure: Mutex::new(None),
            delay: Duration::ZERO,
            feeder_pages: 1,
            queue: tokio::sync::Mutex::new(()),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            listings: AtomicU64::new(0),
            requests: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            hidden: Mutex::new(Vec::new()),
        }
    }
}

impl MockScannerService {
    pub fn new(scanners: Vec<ScannerInfo>, capabilities: ScannerCapabilities) -> Self {
        Self {
            scanners,
            capabilities,
            ..Self::default()
        }
    }

    /// Makes every later call fail with `error`, or succeed again with `None`.
    pub fn fail_with(&self, error: Option<ScanError>) {
        *self.failure.lock().unwrap() = error;
    }

    /// How long each scan takes, so tests can overlap requests.
    pub fn with_scan_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// How many sheets a feeder scan reads, each a page of the sample document. Like SANE,
    /// `scan` keeps only the first of them in PNG and JPEG, `scan_every_page` keeps them all.
    pub fn with_feeder_pages(mut self, pages: usize) -> Self {
        self.feeder_pages = pages;
        self
    }

    /// The most scans ever running at once. The queue keeps it at 1.
    pub fn peak_concurrent_scans(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    /// Every scan request received, in order of arrival at the scanner.
    pub fn scan_requests(&self) -> Vec<(String, ScanRequest)> {
        self.requests.lock().unwrap().clone()
    }

    /// Every id passed to `capabilities` or `scan`, in order: what a real backend would have
    /// tried to open.
    pub fn opened(&self) -> Vec<String> {
        self.opened.lock().unwrap().clone()
    }

    /// The names recto last said it announces its scanners under.
    pub fn hidden(&self) -> Vec<String> {
        self.hidden.lock().unwrap().clone()
    }

    fn check(&self, id: &str) -> Result<(), ScanError> {
        self.opened.lock().unwrap().push(id.to_owned());
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        if self.scanners.iter().any(|s| s.id == id) {
            Ok(())
        } else {
            Err(ScanError::NotFound(id.to_owned()))
        }
    }
}

#[async_trait]
impl ScannerService for MockScannerService {
    async fn scanners(&self, refresh: bool) -> Result<Vec<ScannerInfo>, ScanError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        if refresh {
            self.listings.fetch_add(1, Ordering::SeqCst);
        }
        Ok(self.scanners.clone())
    }

    fn listing(&self) -> u64 {
        self.listings.load(Ordering::SeqCst)
    }

    async fn capabilities(&self, id: &str) -> Result<ScannerCapabilities, ScanError> {
        // SANE answers one request at a time: this waits for a scan in progress, as it does.
        let _turn = self.queue.lock().await;
        self.check(id)?;
        Ok(self.capabilities.clone())
    }

    async fn scan(&self, id: &str, request: ScanRequest) -> Result<Vec<ScannedPage>, ScanError> {
        let every_page = request.format == OutputFormat::Pdf;
        self.scan_pages(id, request, every_page).await
    }

    async fn scan_every_page(
        &self,
        id: &str,
        request: ScanRequest,
    ) -> Result<Vec<ScannedPage>, ScanError> {
        self.scan_pages(id, request, true).await
    }

    fn hide_advertised(&self, names: Vec<String>) {
        *self.hidden.lock().unwrap() = names;
    }
}

impl MockScannerService {
    async fn scan_pages(
        &self,
        id: &str,
        request: ScanRequest,
        every_page: bool,
    ) -> Result<Vec<ScannedPage>, ScanError> {
        let _turn = self.queue.lock().await;
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.requests
            .lock()
            .unwrap()
            .push((id.to_owned(), request.clone()));
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let settings = &request.settings;
        let result = self.check(id).and_then(|()| {
            let area = settings.area.or(self.capabilities.area).unwrap_or(Area {
                left: 0.0,
                top: 0.0,
                right: 210.0,
                bottom: 297.0,
            });
            let image = sample_document(area, settings.resolution, settings.mode);
            let page = ScannedPage::encode(&image, settings.resolution, request.format)?;
            let feeder = settings
                .source
                .as_deref()
                .is_some_and(|s| Source::new(s).is_feeder);
            let sheets = if feeder && every_page && !request.single_page {
                self.feeder_pages
            } else {
                1
            };
            Ok(vec![page; sheets])
        });
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

/// What the mock scanner sees through `area`: a letter on the glass, so previews, selections and
/// screenshots of the development server look like a real scan.
fn sample_document(area: Area, resolution: u32, mode: ScanMode) -> DynamicImage {
    let mm_per_pixel = 25.4 / f64::from(resolution);
    let pixels = |mm: f64| ((mm / mm_per_pixel).round() as u32).max(1);
    let image = RgbImage::from_fn(pixels(area.width()), pixels(area.height()), |x, y| {
        let x = area.left + (f64::from(x) + 0.5) * mm_per_pixel;
        let y = area.top + (f64::from(y) + 0.5) * mm_per_pixel;
        Rgb(sample_document_pixel(x, y))
    });
    let image = DynamicImage::ImageRgb8(image);
    match mode {
        ScanMode::Color => image,
        ScanMode::Gray => DynamicImage::ImageLuma8(image.to_luma8()),
        ScanMode::Lineart => {
            let mut gray = image.to_luma8();
            for pixel in gray.pixels_mut() {
                pixel.0[0] = if pixel.0[0] < 160 { 0 } else { 255 };
            }
            DynamicImage::ImageLuma8(gray)
        }
    }
}

/// The colour at (`x`, `y`) millimetres from the corner of the glass.
fn sample_document_pixel(x: f64, y: f64) -> [u8; 3] {
    const GLASS: [u8; 3] = [214, 216, 220];
    const PAPER: [u8; 3] = [253, 253, 250];
    const ACCENT: [u8; 3] = [21, 101, 192];
    const HEADING: [u8; 3] = [40, 44, 52];
    const TEXT: [u8; 3] = [120, 124, 132];
    // An A4 sheet, slightly off the corner as a hand would lay it.
    let (left, top, right, bottom) = (4.0, 4.0, 214.0, 301.0);
    if !(left..right).contains(&x) || !(top..bottom).contains(&y) {
        return GLASS;
    }
    let (x, y) = (x - left, y - top);
    let inside =
        |x0: f64, y0: f64, x1: f64, y1: f64| (x0..x1).contains(&x) && (y0..y1).contains(&y);
    // Letterhead: a round logo, the sender and a rule under them.
    if (x - 30.0).powi(2) + (y - 30.0).powi(2) < 64.0 {
        return ACCENT;
    }
    if inside(44.0, 25.0, 104.0, 30.0) {
        return HEADING;
    }
    if inside(44.0, 33.0, 84.0, 36.0) || inside(130.0, 25.0, 188.0, 28.0) {
        return TEXT;
    }
    if inside(130.0, 31.0, 172.0, 34.0) || inside(130.0, 37.0, 180.0, 40.0) {
        return TEXT;
    }
    if inside(22.0, 48.0, 188.0, 48.6) {
        return ACCENT;
    }
    if inside(22.0, 62.0, 110.0, 68.0) {
        return HEADING;
    }
    // Paragraphs of grey lines whose ends vary, with a gap every five lines.
    let line_height = 7.0;
    let first = 80.0;
    if (first..250.0).contains(&y) && (22.0..188.0).contains(&x) {
        let row = ((y - first) / line_height) as u32;
        let within = (y - first) - f64::from(row) * line_height;
        let paragraph_end = row % 6 == 5;
        let last_of_paragraph = row % 6 == 4;
        let end = if last_of_paragraph {
            70.0 + f64::from((row * 29) % 60)
        } else {
            188.0 - f64::from((row * 37) % 9)
        };
        if !paragraph_end && within < 2.6 && x < end {
            return TEXT;
        }
    }
    // A signature line.
    if inside(130.0, 268.0, 182.0, 269.0) {
        return HEADING;
    }
    PAPER
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ScanSettings;

    fn settings() -> ScanRequest {
        ScanRequest {
            settings: ScanSettings {
                resolution: 100,
                mode: ScanMode::Gray,
                source: None,
                area: Some(Area {
                    left: 0.0,
                    top: 0.0,
                    right: 50.0,
                    bottom: 25.4,
                }),
            },
            format: OutputFormat::Png,
            single_page: false,
        }
    }

    #[tokio::test]
    async fn generates_a_page_of_the_requested_size() {
        let mock = MockScannerService::default();
        let pages = mock.scan("mock:0", settings()).await.unwrap();
        assert_eq!((pages[0].width, pages[0].height), (197, 100));
        assert!(pages[0].data.starts_with(b"\x89PNG"));
        assert_eq!(pages[0].dpi, 100);
    }

    #[tokio::test]
    async fn follows_the_requested_color_mode() {
        let mock = MockScannerService::default();
        for (mode, gray) in [
            (ScanMode::Color, false),
            (ScanMode::Gray, true),
            (ScanMode::Lineart, true),
        ] {
            let mut request = settings();
            request.settings.mode = mode;
            request.settings.area = None;
            let pages = mock.scan("mock:0", request).await.unwrap();
            assert_eq!(pages[0].gray, gray, "{mode:?}");
        }
    }

    #[tokio::test]
    async fn queued_scans_run_one_at_a_time() {
        let mock =
            Arc::new(MockScannerService::default().with_scan_delay(Duration::from_millis(20)));
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let mock = mock.clone();
                tokio::spawn(async move { mock.scan("mock:0", settings()).await })
            })
            .collect();
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(mock.peak_concurrent_scans(), 1);
        assert_eq!(mock.scan_requests().len(), 4);
    }

    #[tokio::test]
    async fn feeder_batches_keep_every_page_when_asked() {
        let mock = MockScannerService::default().with_feeder_pages(3);
        let mut request = settings();
        request.settings.source = Some("ADF".into());
        let pages = |r| async { mock.scan("mock:0", r).await.unwrap().len() };
        // What the scan page gets: one PNG holds one page.
        assert_eq!(pages(request.clone()).await, 1);
        let every = mock
            .scan_every_page("mock:0", request.clone())
            .await
            .unwrap();
        assert_eq!(every.len(), 3);
        assert!(every.iter().all(|p| p.data.starts_with(b"\x89PNG")));

        let pdf = ScanRequest {
            format: OutputFormat::Pdf,
            ..request.clone()
        };
        assert_eq!(pages(pdf).await, 3);
        let single = ScanRequest {
            single_page: true,
            ..request.clone()
        };
        assert_eq!(
            mock.scan_every_page("mock:0", single).await.unwrap().len(),
            1
        );
        let flatbed = ScanRequest {
            settings: ScanSettings {
                source: Some("Flatbed".into()),
                ..request.settings
            },
            ..request
        };
        assert_eq!(
            mock.scan_every_page("mock:0", flatbed).await.unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn mock_reports_configured_error() {
        let mock = MockScannerService::default();
        mock.fail_with(Some(ScanError::Jammed));
        assert_eq!(
            mock.scan("mock:0", settings()).await.unwrap_err(),
            ScanError::Jammed
        );
        assert_eq!(mock.scanners(false).await.unwrap_err(), ScanError::Jammed);
        mock.fail_with(None);
        assert!(mock.scan("mock:0", settings()).await.is_ok());
    }

    #[tokio::test]
    async fn unknown_scanner_is_not_found() {
        let mock = MockScannerService::default();
        assert_eq!(
            mock.capabilities("x").await.unwrap_err(),
            ScanError::NotFound("x".into())
        );
    }
}
