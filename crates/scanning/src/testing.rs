use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use image::{DynamicImage, GrayImage, Luma};

use crate::{
    Area, ScanError, ScanMode, ScanSettings, ScannedPage, ScannerCapabilities, ScannerInfo,
    ScannerService, Source,
};

/// A scanner stack for tests of the server and the interface: fixed scanners, generated gray
/// pages, optional failure and delay, and a record of what was asked.
pub struct MockScannerService {
    scanners: Vec<ScannerInfo>,
    capabilities: ScannerCapabilities,
    failure: Mutex<Option<ScanError>>,
    delay: Duration,
    /// Scans queue behind this lock like they do behind the SANE worker.
    queue: tokio::sync::Mutex<()>,
    active: AtomicUsize,
    peak: AtomicUsize,
    requests: Mutex<Vec<(String, ScanSettings)>>,
    /// Every id a caller tried to open, listed or not.
    opened: Mutex<Vec<String>>,
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
            queue: tokio::sync::Mutex::new(()),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
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

    /// The most scans ever running at once. The queue keeps it at 1.
    pub fn peak_concurrent_scans(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    /// Every scan request received, in order of arrival at the scanner.
    pub fn scan_requests(&self) -> Vec<(String, ScanSettings)> {
        self.requests.lock().unwrap().clone()
    }

    /// Every id passed to `capabilities` or `scan`, in order: what a real backend would have
    /// tried to open.
    pub fn opened(&self) -> Vec<String> {
        self.opened.lock().unwrap().clone()
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
    async fn scanners(&self, _refresh: bool) -> Result<Vec<ScannerInfo>, ScanError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(self.scanners.clone())
    }

    async fn capabilities(&self, id: &str) -> Result<ScannerCapabilities, ScanError> {
        self.check(id)?;
        Ok(self.capabilities.clone())
    }

    async fn scan(&self, id: &str, settings: ScanSettings) -> Result<Vec<ScannedPage>, ScanError> {
        let _turn = self.queue.lock().await;
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.requests
            .lock()
            .unwrap()
            .push((id.to_owned(), settings.clone()));
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let result = self.check(id).map(|()| {
            let area = settings.area.or(self.capabilities.area).unwrap_or(Area {
                left: 0.0,
                top: 0.0,
                right: 210.0,
                bottom: 297.0,
            });
            let pixels =
                |mm: f64| ((mm * f64::from(settings.resolution) / 25.4).round() as u32).max(1);
            let image =
                GrayImage::from_pixel(pixels(area.width()), pixels(area.height()), Luma([128]));
            vec![ScannedPage {
                image: DynamicImage::ImageLuma8(image),
                dpi: settings.resolution,
            }]
        });
        self.active.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn settings() -> ScanSettings {
        ScanSettings {
            resolution: 100,
            mode: ScanMode::Gray,
            source: None,
            area: Some(Area {
                left: 0.0,
                top: 0.0,
                right: 50.0,
                bottom: 25.4,
            }),
        }
    }

    #[tokio::test]
    async fn generates_a_page_of_the_requested_size() {
        let mock = MockScannerService::default();
        let pages = mock.scan("mock:0", settings()).await.unwrap();
        assert_eq!(
            (pages[0].image.width(), pages[0].image.height()),
            (197, 100)
        );
        assert_eq!(pages[0].dpi, 100);
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
