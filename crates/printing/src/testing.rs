//! Test doubles for the consumers of the printing service.

use std::sync::{Mutex, MutexGuard};

use ipp::attribute::{IppAttribute, IppAttributeGroup};
use ipp::model::{DelimiterTag, IppVersion, StatusCode};
use ipp::request::IppRequestResponse;
use ipp::value::IppValue;

use crate::{
    AdminError, DriverInfo, Job, JobState, JobStatus, Maintenance, NewPrinter, PrintError,
    PrintRequest, PrintService, Printer, PrinterAdmin,
};

/// A valid PDF of `pages` empty pages, for tests that count them.
pub fn sample_pdf(pages: u32) -> Vec<u8> {
    use lopdf::{Document, Object, dictionary};

    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let kids: Vec<Object> = (0..pages)
        .map(|_| {
            document
                .add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                })
                .into()
        })
        .collect();
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Count" => i64::from(pages),
            "Kids" => kids,
        }),
    );
    let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    document.save_to(&mut bytes).expect("write the sample PDF");
    bytes
}

/// A fixed in-memory print service that records what it is asked to do.
#[derive(Default)]
pub struct MockPrintService {
    printers: Mutex<Vec<Printer>>,
    jobs: Mutex<Vec<Job>>,
    submitted: Mutex<Vec<PrintRequest>>,
    canceled: Mutex<Vec<(i32, String)>>,
    impressions: Mutex<Vec<(i32, u32)>>,
    unavailable: Mutex<Option<String>>,
    refusal: Mutex<Option<String>>,
    lost_answer: Mutex<Option<String>>,
    submit_delay: Mutex<std::time::Duration>,
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

    /// Sets what `job_status` reports of a job: its state and the impressions printed.
    pub fn set_job(&self, job_id: i32, state: JobState, impressions: Option<u32>) {
        if let Some(job) = lock(&self.jobs).iter_mut().find(|job| job.id == job_id) {
            job.state = state;
        }
        let mut all = lock(&self.impressions);
        all.retain(|(id, _)| *id != job_id);
        all.extend(impressions.map(|impressions| (job_id, impressions)));
    }

    /// Makes every call fail with `PrintError::Unavailable(reason)` until `set_available`.
    pub fn set_unavailable(&self, reason: &str) {
        *lock(&self.unavailable) = Some(reason.to_owned());
    }

    pub fn set_available(&self) {
        *lock(&self.unavailable) = None;
    }

    /// Makes `submit` (only) fail with `PrintError::Refused(reason)`, as CUPS does when it
    /// answers a job with an error.
    pub fn refuse_submissions(&self, reason: &str) {
        *lock(&self.refusal) = Some(reason.to_owned());
    }

    /// Makes `submit` queue the job and then fail with `PrintError::Unconfirmed(reason)`, as
    /// when CUPS spooled the job and its answer was lost.
    pub fn lose_answers(&self, reason: &str) {
        *lock(&self.lost_answer) = Some(reason.to_owned());
    }

    /// Makes `submit` take `delay` before it queues the job, as a slow CUPS does.
    pub fn delay_submissions(&self, delay: std::time::Duration) {
        *lock(&self.submit_delay) = delay;
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
        let delay = *lock(&self.submit_delay);
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if let Some(reason) = lock(&self.refusal).clone() {
            return Err(PrintError::Refused(reason));
        }
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
            counted_pages: None,
            cost: None,
        });
        lock(&self.submitted).push(request);
        match lock(&self.lost_answer).clone() {
            Some(reason) => Err(PrintError::Unconfirmed(reason)),
            None => Ok(id),
        }
    }

    async fn jobs(&self) -> Result<Vec<Job>, PrintError> {
        self.check_available()?;
        let mut jobs = lock(&self.jobs).clone();
        jobs.sort_unstable_by_key(|job| std::cmp::Reverse(job.id));
        Ok(jobs)
    }

    async fn job_status(&self, job_id: i32) -> Result<Option<JobStatus>, PrintError> {
        self.check_available()?;
        let state = lock(&self.jobs)
            .iter()
            .find(|job| job.id == job_id)
            .map(|job| job.state);
        Ok(state.map(|state| JobStatus {
            state,
            impressions_completed: lock(&self.impressions)
                .iter()
                .find(|(id, _)| *id == job_id)
                .map(|(_, impressions)| *impressions),
        }))
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
    Maintain {
        name: String,
        command: Maintenance,
        by: String,
    },
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

    async fn maintain(&self, name: &str, command: Maintenance, by: &str) -> Result<(), AdminError> {
        self.record(AdminCall::Maintain {
            name: name.to_owned(),
            command,
            by: by.to_owned(),
        })
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
