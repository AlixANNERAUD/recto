//! The printing operations the server needs, so tests can swap CUPS for a mock.

use crate::{Job, JobStatus, PrintError, PrintRequest, Printer};

#[async_trait::async_trait]
pub trait PrintService: Send + Sync + 'static {
    async fn printers(&self) -> Result<Vec<Printer>, PrintError>;

    async fn printer(&self, name: &str) -> Result<Printer, PrintError>;

    /// Sends a document to a printer and returns the job id.
    async fn submit(&self, request: PrintRequest) -> Result<i32, PrintError>;

    /// Jobs that are not completed plus the 20 most recent completed ones, newest first.
    async fn jobs(&self) -> Result<Vec<Job>, PrintError>;

    /// The state of one job, `None` when CUPS no longer knows it.
    async fn job_status(&self, job_id: i32) -> Result<Option<JobStatus>, PrintError>;

    /// Cancels a job. `requested_by` is only used when CUPS does not reveal the job owner.
    async fn cancel(&self, job_id: i32, requested_by: &str) -> Result<(), PrintError>;
}
