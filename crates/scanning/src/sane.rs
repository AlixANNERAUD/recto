use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use async_trait::async_trait;
use libsane::{Constraint, Device, Fixed, Sane, Unit, Value, ValueType, well_known};
use tokio::sync::oneshot;

use crate::{
    OptionConstraint, OptionInfo, OptionUnit, ScanError, ScanSettings, ScannedPage,
    ScannerCapabilities, ScannerInfo, Source, capabilities,
};

use crate::service::ScannerService;

type Reply<T> = oneshot::Sender<Result<T, ScanError>>;

enum Command {
    List(Reply<Vec<ScannerInfo>>),
    Capabilities(String, Reply<ScannerCapabilities>),
    Scan(String, ScanSettings, Reply<Vec<ScannedPage>>),
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
}

impl SaneScanners {
    /// Initialises SANE. Only one instance may exist per process.
    pub fn start() -> Result<Self, ScanError> {
        Ok(Self {
            worker: tokio::sync::Mutex::new(Some(Worker::spawn()?)),
            cache: Mutex::new(None),
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
        if !refresh && let Some(cached) = self.cache.lock().unwrap().clone() {
            return Ok(cached);
        }
        let list = self.ask(Command::List).await?;
        *self.cache.lock().unwrap() = Some(list.clone());
        Ok(list)
    }

    async fn capabilities(&self, id: &str) -> Result<ScannerCapabilities, ScanError> {
        self.ask(|reply| Command::Capabilities(id.to_owned(), reply))
            .await
    }

    async fn scan(&self, id: &str, settings: ScanSettings) -> Result<Vec<ScannedPage>, ScanError> {
        self.ask(|reply| Command::Scan(id.to_owned(), settings, reply))
            .await
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
        Command::Scan(id, settings, reply) => {
            let _ = reply.send(scan(sane, &id, &settings));
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

fn apply(device: &mut Device<'_>, settings: &ScanSettings) -> Result<(), ScanError> {
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
    Ok(())
}

fn scan(sane: &Sane, id: &str, settings: &ScanSettings) -> Result<Vec<ScannedPage>, ScanError> {
    let mut device = open(sane, id)?;
    apply(&mut device, settings)?;
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
        let image = frame.to_image().map_err(|e| map_error(&e))?;
        pages.push(ScannedPage {
            image,
            dpi: settings.resolution,
        });
        if !feeder {
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
