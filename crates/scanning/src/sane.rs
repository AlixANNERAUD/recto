use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use async_trait::async_trait;
use image::{DynamicImage, ImageBuffer};
use libsane::{
    Constraint, Device, Fixed, Frame, FrameFormat, Parameters, Sane, Unit, Value, ValueType,
    well_known,
};
use tokio::sync::oneshot;

use crate::{
    OptionConstraint, OptionInfo, OptionUnit, OutputFormat, ScanError, ScanRequest, ScanSettings,
    ScannedPage, ScannerCapabilities, ScannerInfo, Source, capabilities, check_page_bytes,
};

use crate::escl;
use crate::service::ScannerService;

type Reply<T> = oneshot::Sender<Result<T, ScanError>>;

enum Command {
    List(Reply<Vec<ScannerInfo>>),
    Capabilities(String, Reply<ScannerCapabilities>),
    /// The flag keeps every page of a feeder batch whatever the format.
    Scan(String, ScanRequest, bool, Reply<Vec<ScannedPage>>),
    /// Lets a test prove that a dead worker comes back on the next command. The reply sender
    /// is dropped while unwinding, like the one of a real command that panics.
    #[cfg(test)]
    Crash(Reply<()>),
}
struct Worker {
    commands: Mutex<Option<mpsc::Sender<Command>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Worker {
    /// Blocks until `sane_init` is done, which can be slow with network backends, so callers in
    /// async code run it through `spawn_blocking`.
    fn spawn() -> Result<Arc<Self>, ScanError> {
        let (commands, inbox) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        // libsane keeps devices on the thread that opened them, and `sane_init` must not run
        // twice, so one thread owns everything and the rest talk to it.
        let thread = std::thread::Builder::new()
            .name("recto-sane".into())
            .spawn(move || match Sane::init() {
                Ok(sane) => {
                    let _ = ready_tx.send(Ok(()));
                    for command in inbox {
                        run(&sane, command);
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(map_error(&error)));
                }
            })
            .map_err(|e| ScanError::Device(e.to_string()))?;
        let worker = Arc::new(Self {
            commands: Mutex::new(Some(commands)),
            thread: Mutex::new(Some(thread)),
        });
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(worker),
            Ok(Err(error)) => {
                worker.join();
                Err(error)
            }
            Err(_) => {
                worker.join();
                Err(stopped())
            }
        }
    }

    fn send(&self, command: Command) -> bool {
        // The sender is only borrowed for the call, so it never outlives the request.
        match self.commands.lock().unwrap().as_ref() {
            Some(sender) => sender.send(command).is_ok(),
            None => false,
        }
    }

    /// Closes the inbox and waits for the thread, so its `Sane` is released before another one
    /// is initialised.
    fn join(&self) {
        drop(self.commands.lock().unwrap().take());
        if let Some(thread) = self.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.join();
    }
}

fn stopped() -> ScanError {
    ScanError::Device("scanner worker stopped".into())
}

/// The real scanners, driven through libsane on a dedicated thread.
pub struct SaneScanners {
    /// Async lock: spawning and retiring wait on threads, and other requests must wait for them
    /// without blocking the runtime.
    worker: tokio::sync::Mutex<Option<Arc<Worker>>>,
    cache: Mutex<Option<Vec<ScannerInfo>>>,
    /// Counts the listings read from SANE, for [`ScannerService::listing`].
    listings: AtomicU64,
    /// The DNS-SD instance names recto advertises its scanners under.
    advertised: Mutex<Vec<String>>,
}

impl SaneScanners {
    /// Initialises SANE. Only one instance may exist per process.
    pub fn start() -> Result<Self, ScanError> {
        Ok(Self {
            worker: tokio::sync::Mutex::new(Some(Worker::spawn()?)),
            cache: Mutex::new(None),
            listings: AtomicU64::new(0),
            advertised: Mutex::new(Vec::new()),
        })
    }

    async fn current(&self) -> Result<Arc<Worker>, ScanError> {
        let mut slot = self.worker.lock().await;
        if let Some(worker) = slot.as_ref() {
            return Ok(worker.clone());
        }
        let worker = tokio::task::spawn_blocking(Worker::spawn)
            .await
            .map_err(|_| stopped())??;
        *slot = Some(worker.clone());
        Ok(worker)
    }

    /// Sends a command and waits for the reply. A worker that panicked is replaced on the next
    /// call rather than this one, so the failed request reports the failure honestly.
    async fn ask<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> Result<T, ScanError> {
        let (reply, answer) = oneshot::channel();
        let worker = self.current().await?;
        if !worker.send(build(reply)) {
            self.retire(&worker).await;
            return Err(stopped());
        }
        match answer.await {
            Ok(result) => result,
            Err(_) => {
                self.retire(&worker).await;
                Err(stopped())
            }
        }
    }

    /// Retires `dead` only if it is still the current worker: every request queued on it fails
    /// and lands here, and a later one must not tear down the replacement.
    async fn retire(&self, dead: &Arc<Worker>) {
        let mut slot = self.worker.lock().await;
        if slot.as_ref().is_some_and(|w| Arc::ptr_eq(w, dead)) {
            *slot = None;
            let dead = dead.clone();
            let _ = tokio::task::spawn_blocking(move || dead.join()).await;
        }
    }

    #[cfg(test)]
    async fn crash(&self) -> Result<(), ScanError> {
        self.ask(Command::Crash).await
    }
}

#[async_trait]
impl ScannerService for SaneScanners {
    async fn scanners(&self, refresh: bool) -> Result<Vec<ScannerInfo>, ScanError> {
        let cached = self.cache.lock().unwrap().clone().filter(|_| !refresh);
        let list = match cached {
            Some(cached) => cached,
            None => {
                let list = self.ask(Command::List).await?;
                *self.cache.lock().unwrap() = Some(list.clone());
                self.listings.fetch_add(1, Ordering::Relaxed);
                list
            }
        };
        // Filtered on the way out, so names advertised after the listing apply at once.
        Ok(visible(list, &self.advertised.lock().unwrap()))
    }

    async fn capabilities(&self, id: &str) -> Result<ScannerCapabilities, ScanError> {
        self.ask(|reply| Command::Capabilities(id.to_owned(), reply))
            .await
    }

    async fn scan(&self, id: &str, request: ScanRequest) -> Result<Vec<ScannedPage>, ScanError> {
        let every_page = request.format == OutputFormat::Pdf;
        self.ask(|reply| Command::Scan(id.to_owned(), request, every_page, reply))
            .await
    }

    async fn scan_every_page(
        &self,
        id: &str,
        request: ScanRequest,
    ) -> Result<Vec<ScannedPage>, ScanError> {
        self.ask(|reply| Command::Scan(id.to_owned(), request, true, reply))
            .await
    }

    /// sane-airscan lists a scanner it discovered under its DNS-SD instance name, so devices
    /// named after one of recto's own are left out, along with those whose model starts with
    /// [`escl::MODEL`].
    fn hide_advertised(&self, names: Vec<String>) {
        *self.advertised.lock().unwrap() = names;
    }

    fn listing(&self) -> u64 {
        self.listings.load(Ordering::Relaxed)
    }
}

fn run(sane: &Sane, command: Command) {
    // A closed reply channel means the caller gave up; the work is already done, so ignore it.
    match command {
        Command::List(reply) => {
            let _ = reply.send(list(sane));
        }
        Command::Capabilities(id, reply) => {
            let _ = reply.send(read_capabilities(sane, &id));
        }
        Command::Scan(id, request, every_page, reply) => {
            let _ = reply.send(scan(sane, &id, &request, every_page));
        }
        #[cfg(test)]
        Command::Crash(reply) => {
            let _held = reply;
            panic!("test crash");
        }
    }
}

fn map_error(error: &libsane::Error) -> ScanError {
    use libsane::Error as E;
    match error {
        E::Jammed => ScanError::Jammed,
        E::NoDocs => ScanError::NoDocuments,
        E::CoverOpen => ScanError::CoverOpen,
        E::DeviceBusy => ScanError::Busy,
        other => ScanError::Device(other.to_string()),
    }
}

fn open<'s>(sane: &'s Sane, id: &str) -> Result<Device<'s>, ScanError> {
    sane.open(id).map_err(|e| match e {
        libsane::Error::Invalid => ScanError::NotFound(id.to_owned()),
        other => map_error(&other),
    })
}

/// The devices without recto's own eSCL scanners, which a sane-airscan that discovers them
/// would list: scanning one would loop from SANE to eSCL and back.
fn visible(devices: Vec<ScannerInfo>, advertised: &[String]) -> Vec<ScannerInfo> {
    devices
        .into_iter()
        .filter(|device| !escl::is_own_scanner(&device.model, advertised))
        .collect()
}

fn list(sane: &Sane) -> Result<Vec<ScannerInfo>, ScanError> {
    // Network backends are wanted too: a scanner on the LAN is the common home setup.
    let devices = sane.devices(false).map_err(|e| map_error(&e))?;
    Ok(devices
        .into_iter()
        .map(|d| ScannerInfo {
            id: d.name,
            vendor: d.vendor,
            model: d.model,
            kind: d.kind,
        })
        .collect())
}

fn option_infos(device: &Device<'_>) -> Vec<OptionInfo> {
    device
        .options()
        .iter()
        .map(|o| OptionInfo {
            name: o.name.clone(),
            unit: match o.unit {
                Unit::None => OptionUnit::None,
                Unit::Pixel => OptionUnit::Pixel,
                Unit::Mm => OptionUnit::Mm,
                Unit::Dpi => OptionUnit::Dpi,
                _ => OptionUnit::Other,
            },
            constraint: match &o.constraint {
                Constraint::IntRange(r) => OptionConstraint::IntRange {
                    min: r.min,
                    max: r.max,
                    quant: r.quant,
                },
                Constraint::FixedRange(r) => OptionConstraint::FixedRange {
                    min: r.min.to_f64(),
                    max: r.max.to_f64(),
                    quant: r.quant.to_f64(),
                },
                Constraint::IntList(v) => OptionConstraint::IntList(v.clone()),
                Constraint::FixedList(v) => {
                    OptionConstraint::FixedList(v.iter().map(|f| f.to_f64()).collect())
                }
                Constraint::StringList(v) => OptionConstraint::StringList(v.clone()),
                _ => OptionConstraint::None,
            },
            active: o.is_active(),
            settable: o.is_settable(),
        })
        .collect()
}

fn read_capabilities(sane: &Sane, id: &str) -> Result<ScannerCapabilities, ScanError> {
    let device = open(sane, id)?;
    Ok(capabilities(&option_infos(&device)))
}

fn set(device: &mut Device<'_>, name: &str, value: Value) -> Result<(), ScanError> {
    let info = device.set_value(name, value).map_err(|e| map_error(&e))?;
    // Some backends change other options (ranges, activity) when one is set.
    if info.reload_options() {
        device.reload_options().map_err(|e| map_error(&e))?;
    }
    Ok(())
}

/// The device's own spelling of a string choice, since backends differ in case.
fn choose(device: &Device<'_>, name: &str, wanted: impl Fn(&str) -> bool) -> Option<String> {
    match &device.option(name)?.constraint {
        Constraint::StringList(v) => v.iter().find(|s| wanted(s)).cloned(),
        _ => None,
    }
}

/// Sets the scan's options. Returns the resolution the device will scan at, read back after
/// setting it: a backend may round the one asked for, and a device without the option scans at
/// its own, which the asked one then stands in for. Pages are sized from it.
fn apply(device: &mut Device<'_>, settings: &ScanSettings) -> Result<u32, ScanError> {
    if let Some(source) = &settings.source {
        let exact = choose(device, well_known::SOURCE, |s| {
            s.eq_ignore_ascii_case(source)
        })
        .ok_or_else(|| ScanError::Unsupported(format!("source {source}")))?;
        set(device, well_known::SOURCE, Value::String(exact))?;
    }
    if device.option(well_known::MODE).is_some() {
        let exact = choose(device, well_known::MODE, |s| settings.mode.matches(s))
            .ok_or_else(|| ScanError::Unsupported(format!("mode {}", settings.mode.sane_name())))?;
        set(device, well_known::MODE, Value::String(exact))?;
    }
    if let Some(option) = device.option(well_known::RESOLUTION) {
        let value = match option.value_type {
            ValueType::Fixed => Some(Value::Fixed(Fixed::from_f64(f64::from(
                settings.resolution,
            )))),
            ValueType::Int => i32::try_from(settings.resolution).ok().map(Value::Int),
            _ => None,
        };
        if let Some(value) = value {
            set(device, well_known::RESOLUTION, value)?;
        }
    }
    if let Some(area) = settings.area {
        for (name, mm) in [
            (well_known::TL_X, area.left),
            (well_known::TL_Y, area.top),
            (well_known::BR_X, area.right),
            (well_known::BR_Y, area.bottom),
        ] {
            let Some(option) = device.option(name) else {
                continue;
            };
            if option.unit != Unit::Mm || !option.is_active() || !option.is_settable() {
                continue;
            }
            let value = match option.value_type {
                ValueType::Fixed => Value::Fixed(Fixed::from_f64(mm)),
                ValueType::Int => Value::Int(mm.round() as i32),
                _ => continue,
            };
            set(device, name, value)?;
        }
    }
    let applied = device
        .value(well_known::RESOLUTION)
        .ok()
        .and_then(|value| match value {
            Value::Int(dpi) => u32::try_from(dpi).ok(),
            Value::Fixed(dpi) => Some(dpi.to_f64().round() as u32),
            _ => None,
        })
        .filter(|&dpi| dpi > 0);
    Ok(applied.unwrap_or(settings.resolution))
}

/// The raw size of the image the parameters describe once read, the way [`Frame::to_image`]
/// expands it: a byte per sample (two at 16 bits), three samples per pixel unless gray.
fn image_bytes(parameters: &Parameters, lines: usize) -> u64 {
    let samples: u64 = if parameters.format == FrameFormat::Gray {
        1
    } else {
        3
    };
    let depth: u64 = if parameters.depth == 16 { 2 } else { 1 };
    (parameters.pixels_per_line as u64)
        .saturating_mul(lines as u64)
        .saturating_mul(samples * depth)
}

/// The frame as an image. 8-bit frames without line padding, what scanners send almost always,
/// become the image's buffer as they are: converting them would hold the page twice.
fn into_image(frame: Frame) -> Result<DynamicImage, ScanError> {
    let parameters = frame.parameters;
    let samples = match parameters.format {
        FrameFormat::Gray => 1,
        FrameFormat::Rgb => 3,
        _ => 0,
    };
    if parameters.depth == 8
        && samples > 0
        && parameters.bytes_per_line == frame.width() * samples
        && let (Ok(width), Ok(height)) =
            (u32::try_from(frame.width()), u32::try_from(frame.height()))
    {
        let truncated = || ScanError::Device("the scanner sent a truncated image".into());
        let data = frame.data;
        return if samples == 1 {
            ImageBuffer::from_raw(width, height, data)
                .map(DynamicImage::ImageLuma8)
                .ok_or_else(truncated)
        } else {
            ImageBuffer::from_raw(width, height, data)
                .map(DynamicImage::ImageRgb8)
                .ok_or_else(truncated)
        };
    }
    frame.to_image().map_err(|e| map_error(&e))
}

/// Reads the page, or the feeder batch unless `request` asks for a single page. Without
/// `every_page` only the first page of a batch is kept; the rest of the stack still goes through
/// the feeder, as the scan page says.
fn scan(
    sane: &Sane,
    id: &str,
    request: &ScanRequest,
    every_page: bool,
) -> Result<Vec<ScannedPage>, ScanError> {
    let settings: &ScanSettings = &request.settings;
    let mut device = open(sane, id)?;
    let resolution = apply(&mut device, settings)?;
    // The device's own estimate also covers scanners without geometry options, whose size the
    // server cannot work out from the settings. A feeder that finds the page end itself does
    // not know the line count; the server's estimate from the area is all there is then.
    if let Ok(parameters) = device.parameters()
        && let Some(lines) = parameters.lines
    {
        check_page_bytes(image_bytes(&parameters, lines))?;
    }
    let feeder = match &settings.source {
        Some(source) => Source::new(source).is_feeder,
        None => device
            .value(well_known::SOURCE)
            .ok()
            .and_then(|v| v.as_str().map(|s| Source::new(s).is_feeder))
            .unwrap_or(false),
    };
    let mut scan = device.start().map_err(|e| map_error(&e))?;
    let mut pages = Vec::new();
    loop {
        let frame = scan.read_image().map_err(|e| map_error(&e))?;
        // Each page is compressed before the next one is read, so a feeder batch never sits
        // in memory as raw pixels. This runs on the SANE thread, which is busy with this scan
        // anyway.
        if pages.is_empty() || every_page {
            let image = into_image(frame)?;
            pages.push(ScannedPage::encode(&image, resolution, request.format)?);
        }
        if !feeder || request.single_page {
            break;
        }
        // `next_page` reports an empty feeder as `false`; a no-documents status from the
        // backend after a page is the same thing, not a failure.
        // A failure here drops the pages already read on purpose, so a jam is never hidden.
        match scan.next_page() {
            Ok(true) => {}
            Ok(false) | Err(libsane::Error::NoDocs) => break,
            Err(e) => return Err(map_error(&e)),
        }
    }
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_errors() {
        use libsane::Error as E;
        assert_eq!(map_error(&E::Jammed), ScanError::Jammed);
        assert_eq!(map_error(&E::NoDocs), ScanError::NoDocuments);
        assert_eq!(map_error(&E::CoverOpen), ScanError::CoverOpen);
        assert_eq!(map_error(&E::DeviceBusy), ScanError::Busy);
        assert!(matches!(map_error(&E::Io), ScanError::Device(_)));
    }

    #[test]
    fn recto_never_lists_its_own_scanners() {
        let device = |model: &str| ScannerInfo {
            id: format!("airscan:e0:{model}"),
            vendor: "eSCL".into(),
            model: model.into(),
            kind: "eSCL network scanner".into(),
        };
        let devices = vec![
            device("recto 0.3"),
            device("Office scanner"),
            device("Canon LiDE 300"),
        ];
        let advertised = ["Office scanner".to_owned()];
        assert_eq!(visible(devices, &advertised), [device("Canon LiDE 300")]);
    }

    fn stopped_error() -> ScanError {
        ScanError::Device("scanner worker stopped".into())
    }

    #[tokio::test]
    #[ignore = "needs libsane, see tests/sane_test_backend.rs"]
    async fn worker_restarts_after_a_panic() {
        let sane = SaneScanners::start().unwrap();
        assert!(!sane.scanners(true).await.unwrap().is_empty());
        assert_eq!(sane.crash().await, Err(stopped_error()));
        // The failed command reported the stop; the next one starts a fresh worker.
        assert!(!sane.scanners(true).await.unwrap().is_empty());
    }

    /// Requests queued behind a crashing worker all fail and all retire it. They must not tear
    /// down the replacement, and nothing may block a single-threaded runtime.
    #[tokio::test]
    #[ignore = "needs libsane, see tests/sane_test_backend.rs"]
    async fn requests_queued_on_a_crashing_worker_do_not_kill_its_replacement() {
        use std::sync::Arc;
        use std::time::Duration;

        let sane = Arc::new(SaneScanners::start().unwrap());
        let run = async {
            let crash = tokio::spawn({
                let sane = sane.clone();
                async move { sane.crash().await }
            });
            let queued: Vec<_> = (0..3)
                .map(|_| {
                    let sane = sane.clone();
                    tokio::spawn(async move { sane.scanners(true).await })
                })
                .collect();
            assert_eq!(crash.await.unwrap(), Err(stopped_error()));
            for task in queued {
                let result = task.await.unwrap();
                assert!(
                    result.is_ok() || result == Err(stopped_error()),
                    "{result:?}"
                );
            }
            assert!(!sane.scanners(true).await.unwrap().is_empty());
        };
        tokio::time::timeout(Duration::from_secs(20), run)
            .await
            .expect("no deadlock");
    }
}
