//! Test doubles for the consumers of the printing service.

use std::sync::{Mutex, MutexGuard};

use ipp::attribute::{IppAttribute, IppAttributeGroup};
use ipp::model::{DelimiterTag, IppVersion, StatusCode};
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;

use crate::{
    AdminError, DriverInfo, Job, JobState, NewPrinter, PrintError, PrintRequest, PrintService,
    Printer, PrinterAdmin,
};

/// A fixed in-memory print service that records what it is asked to do.
#[derive(Default)]
pub struct MockPrintService {
    printers: Mutex<Vec<Printer>>,
    jobs: Mutex<Vec<Job>>,
    submitted: Mutex<Vec<PrintRequest>>,
    canceled: Mutex<Vec<(i32, String)>>,
    unavailable: Mutex<Option<String>>,
}

/// A panicking test elsewhere must not hide the state from the assertions of this one.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl MockPrintService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_printers(self, printers: Vec<Printer>) -> Self {
        *lock(&self.printers) = printers;
        self
    }

    pub fn with_jobs(self, jobs: Vec<Job>) -> Self {
        *lock(&self.jobs) = jobs;
        self
    }

    /// Makes every call fail with `PrintError::Unavailable(reason)` until `set_available`.
    pub fn set_unavailable(&self, reason: &str) {
        *lock(&self.unavailable) = Some(reason.to_owned());
    }

    pub fn set_available(&self) {
        *lock(&self.unavailable) = None;
    }

    pub fn submitted(&self) -> Vec<PrintRequest> {
        lock(&self.submitted).clone()
    }

    /// Canceled `(job id, requested_by)` pairs.
    pub fn canceled(&self) -> Vec<(i32, String)> {
        lock(&self.canceled).clone()
    }

    fn check_available(&self) -> Result<(), PrintError> {
        match lock(&self.unavailable).as_ref() {
            Some(reason) => Err(PrintError::Unavailable(reason.clone())),
            None => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl PrintService for MockPrintService {
    async fn printers(&self) -> Result<Vec<Printer>, PrintError> {
        self.check_available()?;
        Ok(lock(&self.printers).clone())
    }

    async fn printer(&self, name: &str) -> Result<Printer, PrintError> {
        self.check_available()?;
        lock(&self.printers)
            .iter()
            // CUPS finds a printer whatever the case of its name.
            .find(|printer| printer.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| PrintError::PrinterNotFound(name.to_owned()))
    }

    async fn submit(&self, request: PrintRequest) -> Result<i32, PrintError> {
        self.check_available()?;
        if !lock(&self.printers)
            .iter()
            .any(|p| p.name == request.printer)
        {
            return Err(PrintError::PrinterNotFound(request.printer));
        }
        let mut jobs = lock(&self.jobs);
        let id = jobs.iter().map(|job| job.id).max().unwrap_or(0) + 1;
        jobs.push(Job {
            id,
            printer: request.printer.clone(),
            name: Some(request.job_name.clone()),
            owner: Some(request.user.clone()),
            state: JobState::Pending,
            pages: None,
            created: None,
        });
        lock(&self.submitted).push(request);
        Ok(id)
    }

    async fn jobs(&self) -> Result<Vec<Job>, PrintError> {
        self.check_available()?;
        let mut jobs = lock(&self.jobs).clone();
        jobs.sort_unstable_by_key(|job| std::cmp::Reverse(job.id));
        Ok(jobs)
    }

    async fn cancel(&self, job_id: i32, requested_by: &str) -> Result<(), PrintError> {
        self.check_available()?;
        let mut jobs = lock(&self.jobs);
        let job = jobs
            .iter_mut()
            .find(|job| job.id == job_id)
            .ok_or(PrintError::JobNotFound(job_id))?;
        job.state = JobState::Canceled;
        lock(&self.canceled).push((job_id, requested_by.to_owned()));
        Ok(())
    }
}

/// What a [`MockPrinterAdmin`] was asked, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminCall {
    Add(NewPrinter),
    Describe {
        name: String,
        description: Option<String>,
        location: Option<String>,
    },
    SetDefault(String),
    Pause(String),
    Resume(String),
    Accept(String),
    Reject(String),
    Delete(String),
    Drivers {
        search: String,
        limit: u32,
    },
}

/// A printer administration that records what it is asked and changes nothing.
#[derive(Default)]
pub struct MockPrinterAdmin {
    calls: Mutex<Vec<AdminCall>>,
    drivers: Mutex<Vec<DriverInfo>>,
    failure: Mutex<Option<AdminError>>,
}

impl MockPrinterAdmin {
    pub fn new() -> Self {
        Self::default()
    }

    /// The drivers `drivers` searches in.
    pub fn with_drivers(self, drivers: Vec<DriverInfo>) -> Self {
        *lock(&self.drivers) = drivers;
        self
    }

    /// Makes every call fail with `error` (after recording it) until `succeed`.
    pub fn fail_with(&self, error: AdminError) {
        *lock(&self.failure) = Some(error);
    }

    pub fn succeed(&self) {
        *lock(&self.failure) = None;
    }

    pub fn calls(&self) -> Vec<AdminCall> {
        lock(&self.calls).clone()
    }

    fn record(&self, call: AdminCall) -> Result<(), AdminError> {
        lock(&self.calls).push(call);
        match lock(&self.failure).clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl PrinterAdmin for MockPrinterAdmin {
    async fn add(&self, printer: NewPrinter) -> Result<(), AdminError> {
        self.record(AdminCall::Add(printer))
    }

    async fn describe(
        &self,
        name: &str,
        description: Option<String>,
        location: Option<String>,
    ) -> Result<(), AdminError> {
        self.record(AdminCall::Describe {
            name: name.to_owned(),
            description,
            location,
        })
    }

    async fn set_default(&self, name: &str) -> Result<(), AdminError> {
        self.record(AdminCall::SetDefault(name.to_owned()))
    }

    async fn pause(&self, name: &str) -> Result<(), AdminError> {
        self.record(AdminCall::Pause(name.to_owned()))
    }

    async fn resume(&self, name: &str) -> Result<(), AdminError> {
        self.record(AdminCall::Resume(name.to_owned()))
    }

    async fn accept(&self, name: &str) -> Result<(), AdminError> {
        self.record(AdminCall::Accept(name.to_owned()))
    }

    async fn reject(&self, name: &str) -> Result<(), AdminError> {
        self.record(AdminCall::Reject(name.to_owned()))
    }

    async fn delete(&self, name: &str) -> Result<(), AdminError> {
        self.record(AdminCall::Delete(name.to_owned()))
    }

    async fn drivers(&self, search: &str, limit: u32) -> Result<Vec<DriverInfo>, AdminError> {
        self.record(AdminCall::Drivers {
            search: search.to_owned(),
            limit,
        })?;
        let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
        Ok(lock(&self.drivers)
            .iter()
            .filter(|driver| {
                let model = driver.make_and_model.to_lowercase();
                words.iter().all(|word| model.contains(word))
            })
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .cloned()
            .collect())
    }
}

/// An attribute with a name known to be valid.
pub fn attribute(name: &str, value: IppValue) -> IppAttribute {
    IppAttribute::with_name(name, value).expect("valid attribute name")
}

/// The bytes of an IPP response. Every `(tag, attributes)` pair becomes its own group, as CUPS
/// sends one group per printer or job.
pub fn ipp_response(status: StatusCode, groups: Vec<(DelimiterTag, Vec<IppAttribute>)>) -> Vec<u8> {
    let mut response = IppRequestResponse::new_response(IppVersion::v1_1(), status, 1)
        .expect("valid response header");
    for (tag, attributes) in groups {
        let mut group = IppAttributeGroup::new(tag);
        group.attributes_mut().extend(attributes);
        response.attributes_mut().groups_mut().push(group);
    }
    response.to_bytes().to_vec()
}
