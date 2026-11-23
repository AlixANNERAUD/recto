//! eSCL jobs: what a client created with `POST ScanJobs`, from the scan to its last document.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use uuid::Uuid;

use super::{AdfState, EsclJobState, EsclState, ImageInfo, JobStatusView};
use crate::{OutputFormat, ScanError, ScannedPage, assemble};

/// How long a finished job stays, for `NextDocument`, `ScannerStatus` and `ScanImageInfo`.
const LIFETIME: Duration = Duration::from_secs(10 * 60);

/// Whose a job is: only they list it, fetch its pages or cancel it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// A device password's user.
    User(i64),
    /// A client of an open network, without credentials, by its address: two people scanning
    /// anonymously must not see or take each other's pages.
    Anonymous(IpAddr),
}

/// One document of a job: the PDF, or one PNG or JPEG page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub info: ImageInfo,
}

/// The answer to `NextDocument`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextDocument {
    Document(Document),
    /// Every document was handed out. HTTP 404, which ends the job for the client.
    Done,
    /// Still scanning after the wait. HTTP 503 with `Retry-After`.
    NotYet,
    /// The feeder was empty. HTTP 404, with `ScannerAdfEmpty` in the status.
    NoDocuments,
    /// The scan failed: 409 for a jam or an open cover, 503 while busy, 500 otherwise.
    Failed(ScanError),
    /// No such job for this caller, or canceled. HTTP 404.
    NotFound,
}

enum Outcome {
    Scanning,
    Ready(VecDeque<Document>),
    Failed(ScanError),
    Canceled,
}

struct Job {
    owner: Owner,
    scanner: String,
    format: OutputFormat,
    /// Orders the jobs newest first.
    sequence: u64,
    changed: Instant,
    outcome: Outcome,
    /// Until SANE gives the scan back, whatever the outcome: a canceled job's scanner is still
    /// reading its page, and the next scan would only queue behind it.
    busy: bool,
    fetched: u32,
    last: Option<ImageInfo>,
}

impl Job {
    fn state(&self) -> EsclJobState {
        match &self.outcome {
            Outcome::Scanning => EsclJobState::Processing,
            Outcome::Ready(documents) if !documents.is_empty() => EsclJobState::Processing,
            Outcome::Ready(_) if self.fetched > 0 => EsclJobState::Completed,
            Outcome::Ready(_) | Outcome::Failed(_) => EsclJobState::Aborted,
            Outcome::Canceled => EsclJobState::Canceled,
        }
    }
}

impl Jobs {
    /// Whether SANE is still scanning for one of the scanner's eSCL jobs.
    fn busy(&self, scanner: &str) -> bool {
        self.jobs
            .values()
            .any(|job| job.scanner == scanner && job.busy)
    }
}

#[derive(Default)]
struct Jobs {
    jobs: HashMap<Uuid, Job>,
    /// The feeder of each scanner, by key, after a job found it empty, jammed or open.
    feeders: HashMap<String, AdfState>,
    sequence: u64,
}

/// The eSCL jobs of every scanner, in memory: they only live as long as a client fetches them.
/// Each belongs to the user who created it (`None` for anonymous clients), and nobody else sees
/// it. A scanner runs one eSCL job at a time; web scans queue in the scanner service as usual.
pub struct EsclJobs {
    state: Mutex<Jobs>,
    /// Wakes `next_document` callers when a job changes.
    changed: Notify,
    lifetime: Duration,
}

impl Default for EsclJobs {
    fn default() -> Self {
        Self::with_lifetime(LIFETIME)
    }
}

impl EsclJobs {
    /// Jobs forgotten `lifetime` after their last change, rather than 10 minutes.
    pub fn with_lifetime(lifetime: Duration) -> Self {
        Self {
            state: Mutex::new(Jobs::default()),
            changed: Notify::new(),
            lifetime,
        }
    }

    /// The jobs, without those past their lifetime. A scan still running is kept whatever its
    /// age, canceled or not: its scanner stays busy until it returns.
    fn lock(&self) -> MutexGuard<'_, Jobs> {
        // Every update leaves the map whole, so a panic elsewhere cannot have broken it.
        let mut jobs = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let lifetime = self.lifetime;
        jobs.jobs
            .retain(|_, job| job.busy || job.changed.elapsed() < lifetime);
        jobs
    }

    /// Creates a job for `scanner` (its key), or `None` while an eSCL job of that scanner is
    /// still scanning, canceled or not (HTTP 503). The caller runs the scan and must call
    /// [`finish`](Self::finish) whatever happens, or the scanner stays busy.
    pub fn start(&self, owner: Owner, scanner: &str, format: OutputFormat) -> Option<Uuid> {
        let mut jobs = self.lock();
        if jobs.busy(scanner) {
            return None;
        }
        let id = Uuid::new_v4();
        jobs.sequence += 1;
        let sequence = jobs.sequence;
        jobs.feeders.remove(scanner);
        jobs.jobs.insert(
            id,
            Job {
                owner,
                scanner: scanner.to_owned(),
                format,
                sequence,
                changed: Instant::now(),
                outcome: Outcome::Scanning,
                busy: true,
                fetched: 0,
                last: None,
            },
        );
        Some(id)
    }

    /// Hands the scan's result to the job, and frees its scanner: a PDF becomes one document,
    /// PNG and JPEG pages one each. The pages of a canceled or forgotten job are dropped.
    pub fn finish(&self, id: Uuid, result: Result<Vec<ScannedPage>, ScanError>) {
        let format = {
            let mut jobs = self.lock();
            let Some(job) = jobs.jobs.get_mut(&id) else {
                return;
            };
            if !matches!(job.outcome, Outcome::Scanning) {
                job.busy = false;
                return;
            }
            job.format
        };
        // Outside the lock: a PDF of a long batch takes a moment to write.
        let result = result.and_then(|pages| documents(pages, format));
        let mut jobs = self.lock();
        let Jobs { jobs, feeders, .. } = &mut *jobs;
        let Some(job) = jobs.get_mut(&id) else {
            return;
        };
        job.busy = false;
        // Canceled while the PDF was written.
        if !matches!(job.outcome, Outcome::Scanning) {
            return;
        }
        let feeder = match &result {
            Err(ScanError::NoDocuments) => Some(AdfState::Empty),
            Err(ScanError::Jammed) => Some(AdfState::Jam),
            Err(ScanError::CoverOpen) => Some(AdfState::DoorOpen),
            _ => None,
        };
        if let Some(feeder) = feeder {
            feeders.insert(job.scanner.clone(), feeder);
        }
        job.outcome = match result {
            Ok(documents) => Outcome::Ready(documents.into()),
            Err(error) => Outcome::Failed(error),
        };
        job.changed = Instant::now();
        self.changed.notify_waiters();
    }

    /// The job's next document, waiting up to `wait` for the scan to finish.
    pub async fn next_document(&self, owner: Owner, id: Uuid, wait: Duration) -> NextDocument {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Registered before looking, so a `finish` in between is not missed.
            let mut changed = std::pin::pin!(self.changed.notified());
            changed.as_mut().enable();
            if let Some(answer) = self.take_next(owner, id) {
                return answer;
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return NextDocument::NotYet;
            }
        }
    }

    /// The next document, or `None` while the job is scanning.
    fn take_next(&self, owner: Owner, id: Uuid) -> Option<NextDocument> {
        let mut jobs = self.lock();
        let Some(job) = jobs.jobs.get_mut(&id).filter(|job| job.owner == owner) else {
            return Some(NextDocument::NotFound);
        };
        let answer = match &mut job.outcome {
            Outcome::Scanning => return None,
            Outcome::Ready(documents) => match documents.pop_front() {
                Some(document) => {
                    job.fetched += 1;
                    job.last = Some(document.info);
                    job.changed = Instant::now();
                    NextDocument::Document(document)
                }
                None if job.fetched == 0 => NextDocument::NoDocuments,
                None => NextDocument::Done,
            },
            Outcome::Failed(ScanError::NoDocuments) => NextDocument::NoDocuments,
            Outcome::Failed(error) => NextDocument::Failed(error.clone()),
            Outcome::Canceled => NextDocument::NotFound,
        };
        Some(answer)
    }

    /// `DELETE` of a job: a running scan is marked canceled and its pages dropped when they
    /// arrive (SANE cannot stop mid-page); a finished job is forgotten. False when the caller
    /// has no such job.
    pub fn cancel(&self, owner: Owner, id: Uuid) -> bool {
        let mut jobs = self.lock();
        let Some(job) = jobs.jobs.get_mut(&id).filter(|job| job.owner == owner) else {
            return false;
        };
        match job.outcome {
            Outcome::Scanning => {
                job.outcome = Outcome::Canceled;
                job.changed = Instant::now();
            }
            Outcome::Canceled => {}
            _ => {
                jobs.jobs.remove(&id);
            }
        }
        self.changed.notify_waiters();
        true
    }

    /// The scanner's state: processing while one of its eSCL jobs scans, canceled or not.
    pub fn state(&self, scanner: &str) -> EsclState {
        if self.lock().busy(scanner) {
            EsclState::Processing
        } else {
            EsclState::Idle
        }
    }

    /// The scanner's feeder as its last job left it, loaded until a job finds otherwise.
    pub fn adf_state(&self, scanner: &str) -> AdfState {
        self.lock()
            .feeders
            .get(scanner)
            .copied()
            .unwrap_or(AdfState::Loaded)
    }

    /// The caller's jobs on `scanner`, newest first, for `ScannerStatus`.
    pub fn jobs(&self, owner: Owner, scanner: &str) -> Vec<JobStatusView> {
        let jobs = self.lock();
        let mut mine: Vec<(&Uuid, &Job)> = jobs
            .jobs
            .iter()
            .filter(|(_, job)| job.owner == owner && job.scanner == scanner)
            .collect();
        mine.sort_by_key(|(_, job)| std::cmp::Reverse(job.sequence));
        mine.into_iter()
            .map(|(id, job)| JobStatusView {
                id: *id,
                state: job.state(),
                age_seconds: job.changed.elapsed().as_secs(),
                images_completed: job.fetched,
                images_to_transfer: match &job.outcome {
                    Outcome::Ready(documents) => u32::try_from(documents.len()).unwrap_or(u32::MAX),
                    _ => 0,
                },
            })
            .collect()
    }

    /// The size of the last document the caller fetched from the job, for `ScanImageInfo`.
    pub fn image_info(&self, owner: Owner, id: Uuid) -> Option<ImageInfo> {
        self.lock()
            .jobs
            .get(&id)
            .filter(|job| job.owner == owner)
            .and_then(|job| job.last)
    }
}

/// The documents of a scan: one PDF for every page, or one image per page.
fn documents(pages: Vec<ScannedPage>, format: OutputFormat) -> Result<Vec<Document>, ScanError> {
    let info = |page: &ScannedPage| ImageInfo {
        width: page.width,
        height: page.height,
        gray: page.gray,
    };
    match (format, pages.first()) {
        (_, None) => Ok(Vec::new()),
        (OutputFormat::Pdf, Some(first)) => {
            let info = info(first);
            Ok(vec![Document {
                bytes: assemble(pages, OutputFormat::Pdf)?,
                mime: OutputFormat::Pdf.mime(),
                info,
            }])
        }
        (_, Some(_)) => Ok(pages
            .into_iter()
            .map(|page| Document {
                info: info(&page),
                mime: page.format.mime(),
                bytes: page.data,
            })
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use image::{DynamicImage, GrayImage, RgbImage};

    use super::*;
    use crate::escl::EsclJobState;

    const ALICE: Owner = Owner::User(1);
    const BOB: Owner = Owner::User(2);
    const PHONE: Owner = Owner::Anonymous(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 20)));
    const TABLET: Owner = Owner::Anonymous(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 21)));
    const KEY: &str = "1a2b3c4d";
    const LONG: Duration = Duration::from_secs(5);

    fn page(width: u32, format: OutputFormat) -> ScannedPage {
        let image = DynamicImage::ImageRgb8(RgbImage::new(width, 10));
        ScannedPage::encode(&image, 75, format).unwrap()
    }

    fn document(next: NextDocument) -> Document {
        match next {
            NextDocument::Document(document) => document,
            other => panic!("expected a document, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn next_document_waits_then_delivers() {
        let jobs = Arc::new(EsclJobs::default());
        let id = jobs.start(ALICE, KEY, OutputFormat::Jpeg).unwrap();
        let finisher = tokio::spawn({
            let jobs = jobs.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                jobs.finish(
                    id,
                    Ok(vec![
                        page(20, OutputFormat::Jpeg),
                        page(30, OutputFormat::Jpeg),
                    ]),
                );
            }
        });
        let first = document(jobs.next_document(ALICE, id, LONG).await);
        assert_eq!(first.mime, "image/jpeg");
        assert!(first.bytes.starts_with(&[0xFF, 0xD8]));
        assert_eq!(first.info.width, 20);
        assert_eq!(jobs.image_info(ALICE, id).unwrap().width, 20);
        // Another person's job does not exist for them.
        assert_eq!(
            jobs.next_document(BOB, id, LONG).await,
            NextDocument::NotFound
        );
        assert_eq!(
            jobs.next_document(PHONE, id, LONG).await,
            NextDocument::NotFound
        );
        assert_eq!(jobs.image_info(BOB, id), None);
        assert!(!jobs.cancel(BOB, id));

        let status = jobs.jobs(ALICE, KEY);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].state, EsclJobState::Processing);
        assert_eq!(
            (status[0].images_completed, status[0].images_to_transfer),
            (1, 1)
        );

        let second = document(jobs.next_document(ALICE, id, LONG).await);
        assert_eq!(second.info.width, 30);
        assert_eq!(
            jobs.next_document(ALICE, id, LONG).await,
            NextDocument::Done
        );
        assert_eq!(jobs.jobs(ALICE, KEY)[0].state, EsclJobState::Completed);
        finisher.await.unwrap();
    }

    #[tokio::test]
    async fn a_pdf_is_one_document() {
        let jobs = EsclJobs::default();
        let id = jobs.start(PHONE, KEY, OutputFormat::Pdf).unwrap();
        jobs.finish(
            id,
            Ok(vec![
                page(20, OutputFormat::Pdf),
                page(30, OutputFormat::Pdf),
            ]),
        );
        let pdf = document(jobs.next_document(PHONE, id, LONG).await);
        assert_eq!(pdf.mime, "application/pdf");
        assert!(pdf.bytes.starts_with(b"%PDF-"));
        assert_eq!(
            jobs.next_document(PHONE, id, LONG).await,
            NextDocument::Done
        );
    }

    #[tokio::test]
    async fn a_slow_scan_is_not_ready_yet() {
        let jobs = EsclJobs::default();
        let id = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        let wait = Duration::from_millis(20);
        assert_eq!(
            jobs.next_document(ALICE, id, wait).await,
            NextDocument::NotYet
        );
        assert_eq!(jobs.state(KEY), EsclState::Processing);
        assert_eq!(jobs.state("other"), EsclState::Idle);
        let gray = DynamicImage::ImageLuma8(GrayImage::new(8, 8));
        jobs.finish(
            id,
            Ok(vec![
                ScannedPage::encode(&gray, 75, OutputFormat::Png).unwrap(),
            ]),
        );
        assert_eq!(jobs.state(KEY), EsclState::Idle);
        let png = document(jobs.next_document(ALICE, id, wait).await);
        assert_eq!(png.mime, "image/png");
        assert!(png.info.gray);
    }

    #[test]
    fn one_job_at_a_time_per_scanner() {
        let jobs = EsclJobs::default();
        let id = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        assert_eq!(jobs.start(BOB, KEY, OutputFormat::Png), None);
        assert!(jobs.start(BOB, "other", OutputFormat::Png).is_some());
        jobs.finish(id, Ok(vec![page(8, OutputFormat::Png)]));
        assert!(jobs.start(BOB, KEY, OutputFormat::Png).is_some());
    }

    #[tokio::test]
    async fn feeder_failures_show_in_its_state_until_the_next_job() {
        let jobs = EsclJobs::default();
        assert_eq!(jobs.adf_state(KEY), AdfState::Loaded);
        let id = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        jobs.finish(id, Err(ScanError::NoDocuments));
        assert_eq!(
            jobs.next_document(ALICE, id, LONG).await,
            NextDocument::NoDocuments
        );
        assert_eq!(jobs.adf_state(KEY), AdfState::Empty);
        assert_eq!(jobs.jobs(ALICE, KEY)[0].state, EsclJobState::Aborted);

        for (error, state) in [
            (ScanError::Jammed, AdfState::Jam),
            (ScanError::CoverOpen, AdfState::DoorOpen),
        ] {
            let id = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
            assert_eq!(jobs.adf_state(KEY), AdfState::Loaded);
            jobs.finish(id, Err(error.clone()));
            assert_eq!(
                jobs.next_document(ALICE, id, LONG).await,
                NextDocument::Failed(error)
            );
            assert_eq!(jobs.adf_state(KEY), state);
        }
        let id = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        jobs.finish(id, Err(ScanError::Device("io".into())));
        assert_eq!(jobs.adf_state(KEY), AdfState::Loaded);
        // Newest first.
        let listed = jobs.jobs(ALICE, KEY);
        assert_eq!(listed.len(), 4);
        assert_eq!(listed[0].id, id);
    }

    #[tokio::test]
    async fn a_canceled_scan_drops_its_pages() {
        let jobs = EsclJobs::default();
        let id = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        assert!(jobs.cancel(ALICE, id));
        // SANE cannot stop mid-page: the scanner stays busy until the scan returns, or deleting
        // each job at once would queue scan after scan.
        assert_eq!(jobs.state(KEY), EsclState::Processing);
        assert_eq!(jobs.start(BOB, KEY, OutputFormat::Png), None);
        assert_eq!(jobs.jobs(ALICE, KEY)[0].state, EsclJobState::Canceled);
        jobs.finish(id, Ok(vec![page(8, OutputFormat::Png)]));
        assert_eq!(jobs.state(KEY), EsclState::Idle);
        assert_eq!(
            jobs.next_document(ALICE, id, LONG).await,
            NextDocument::NotFound
        );
        assert_eq!(jobs.jobs(ALICE, KEY)[0].state, EsclJobState::Canceled);
        assert_eq!(jobs.jobs(ALICE, KEY)[0].images_to_transfer, 0);
        // Deleting a finished job forgets it.
        let done = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        jobs.finish(done, Ok(vec![page(8, OutputFormat::Png)]));
        assert!(jobs.cancel(ALICE, done));
        assert!(!jobs.cancel(ALICE, done));
        assert_eq!(jobs.jobs(ALICE, KEY).len(), 1);
        assert!(!jobs.cancel(ALICE, Uuid::nil()));
    }

    #[tokio::test]
    async fn jobs_are_listed_to_their_owner_only() {
        let jobs = EsclJobs::default();
        let alice = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        jobs.finish(alice, Ok(vec![page(8, OutputFormat::Png)]));
        let anonymous = jobs.start(PHONE, KEY, OutputFormat::Png).unwrap();
        assert_eq!(
            jobs.jobs(ALICE, KEY)
                .iter()
                .map(|j| j.id)
                .collect::<Vec<_>>(),
            [alice]
        );
        assert_eq!(
            jobs.jobs(PHONE, KEY)
                .iter()
                .map(|j| j.id)
                .collect::<Vec<_>>(),
            [anonymous]
        );
        assert!(jobs.jobs(BOB, KEY).is_empty());
        assert!(jobs.jobs(TABLET, KEY).is_empty());
        assert!(!jobs.cancel(TABLET, anonymous));
        assert!(jobs.jobs(ALICE, "other").is_empty());
    }

    #[tokio::test]
    async fn finished_jobs_expire() {
        let jobs = EsclJobs::with_lifetime(Duration::from_millis(30));
        let finished = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        jobs.finish(finished, Ok(vec![page(8, OutputFormat::Png)]));
        let scanning = jobs.start(ALICE, "other", OutputFormat::Png).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            jobs.next_document(ALICE, finished, LONG).await,
            NextDocument::NotFound
        );
        // A scan still running is never forgotten: its pages are on their way. Nor is a
        // canceled one SANE has not given back: its scanner is still busy.
        assert_eq!(jobs.jobs(ALICE, "other")[0].id, scanning);
        let canceled = jobs.start(ALICE, KEY, OutputFormat::Png).unwrap();
        assert!(jobs.cancel(ALICE, canceled));
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(jobs.state(KEY), EsclState::Processing);
        jobs.finish(canceled, Err(ScanError::Interrupted));
        assert!(jobs.start(ALICE, KEY, OutputFormat::Png).is_some());
        jobs.finish(scanning, Ok(vec![page(8, OutputFormat::Png)]));
        assert!(matches!(
            jobs.next_document(ALICE, scanning, LONG).await,
            NextDocument::Document(_)
        ));
    }
}
